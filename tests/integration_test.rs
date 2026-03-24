use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use testcontainers::core::BuildImageOptions;
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

fn mk_img() {
    let image = GenericBuildableImage::new("my-test-app", "latest")
        .with_dockerfile_string(
            r#"
            FROM alpine:latest
            COPY --chmod=0755 app.sh /usr/local/bin/app
            ENTRYPOINT ["/usr/local/bin/app"]
        "#,
        )
        .with_data(
            r#"#!/bin/sh
echo "Hello from custom image!"
"#,
            "./app.sh",
        )
        .build_image_with(BuildImageOptions::new().with_skip_if_exists(true));
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

    run_asm(asm).expect("program should exit successfully");
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

    let result = run_asm(asm);
    assert!(result.is_err(), "expected failure for non-zero exit");
}
