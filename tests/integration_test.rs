use std::fs::read_to_string;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use testcontainers::core::{BuildImageOptions, CmdWaitFor, ExecCommand, WaitFor};
use testcontainers::runners::{SyncBuilder, SyncRunner};
use testcontainers::{Container, GenericBuildableImage, GenericImage, ImageExt};

// Problems:
// - Keeps making new containers per run, and never cleans up
// - Fails to copy ASM for the second test
// - Ubuntu is too chonky. we can assemble and link elf32 binary locally and just run that. We don't need GNU utils probably

// should definitely not be a container
static CONTAINER: LazyLock<Container<GenericImage>> = LazyLock::new(|| {
    // rely on default entrypoint (bash exists in ubuntu)
    // TODO: can likely get away with less of a linux install
    // TODO: GenericImage is deprecated???
    let image = GenericImage::new("alpine", "latest").with_working_dir("/work");

    image.start().expect("failed to start container")
});

// Make the image async
//          add a bash script to run the binaries and capture output
//
// Separately compile all the tests that compile
//
// now copy them to the image, spin up the container, and then run
// copy the results back
//
// check them with expected results

fn mk_img(source: &str) -> Option<i64> {
    let image = GenericBuildableImage::new("my-test-app", "latest")
        .with_dockerfile_string(
            r#"
            FROM alpine:latest
            RUN mkdir /work
            COPY --chmod=0755 app.s /
            COPY --chmod=0755 entrypoint.sh /
            COPY --chmod=0755 test /
            ENTRYPOINT ["/entrypoint.sh"]
        "#,
        )
        .with_data(source, "./app.s")
        .with_data(std::fs::read("./test").expect("???"), "./test")
        .with_data(
            r#"#!/bin/sh
echo "started"
cat app.s
./test
echo $?
cat
"#,
            "./entrypoint.sh",
        )
        .build_image();
    // .build_image_with(BuildImageOptions::new().with_skip_if_exists(true));

    let container = image
        .expect("failed to create image")
        .start()
        .expect("failed to start container");

    let cmd = ExecCommand::new(vec![
        "nasm -f elf64 app.s -o app.o && ld app.o -o app.out && ./a.out",
    ])
    .with_container_ready_conditions(vec![WaitFor::message_on_stdout("started")]);
    // .with_cmd_ready_condition(CmdWaitFor::Exit { code: None });

    eprintln!("Reached here");
    let result = container.exec(cmd);
    result
        .expect("failed to run command")
        .exit_code()
        .expect("failed to get exit code")
}

// fails if the program exits non-zero
fn run_asm(source: &str) -> Result<(), String> {
    let container = &*CONTAINER;
    let id = container.id();

    let filename = format!("test-{}.s", std::process::id());

    // write ASM into container using stdin
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            id,
            "bash",
            "-c",
            &format!("cat > /work/{filename}"),
        ])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(source.as_bytes())
        .map_err(|e| e.to_string())?;

    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("failed to write ASM into container".into());
    }

    let output = Command::new("docker")
        .args([
            "exec",
            id,
            "bash",
            "-c",
            &format!(
                "cd /work && \
                 nasm -f elf64 {0} -o {0}.o && \
                 ld {0}.o -o {0}.out && \
                 ./{0}.out",
                filename
            ),
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(format!(
            "Execution failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ));
    }

    Ok(())
}

#[test]
fn test_exit_zero() {
    let asm = r#"
global _start

section .text
_start:
    mov rax, 60
    mov rdi, 0
    syscall
"#;

    let code = mk_img(asm);
    assert!(code.is_some());
}

#[test]
fn test_exit_nonzero() {
    let asm = r#"
global _start

section .text
_start:
    mov rax, 60
    mov rdi, 42
    syscall
"#;

    let code = mk_img(asm);
    assert!(code.is_some());
}
