use crate::protobufs::setup_protobufs;
use std::{error::Error, path::PathBuf};

pub mod protobufs;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build/main.rs");
    println!("cargo::rerun-if-changed=build/protobufs.rs");
    println!("cargo::rerun-if-changed=proto/");

    let base = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    setup_protobufs(base);
    Ok(())
}
