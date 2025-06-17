use std::process::{Command, Stdio};

fn main() {
    // Rebuild if frontend files change
    println!("cargo:rerun-if-changed=../../Trunk.toml");
    println!("cargo:rerun-if-changed=../../frontend/");

    // Check if Trunk is installed
    if Command::new("trunk")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        println!(
            "cargo:warning=Trunk not found - frontend won't be built install trunk with \"cargo install trunk\""
        );
        return;
    }

    // Run Trunk build
    let status = Command::new("trunk")
        .current_dir("../..")
        .arg("build")
        .status()
        .expect("Failed to execute Trunk");

    if !status.success() {
        panic!("Trunk build failed");
    }
}
