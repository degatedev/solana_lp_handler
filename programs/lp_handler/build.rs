use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=SECURITY_ADMIN");

    let security_admin = env::var("SECURITY_ADMIN")
        .expect("SECURITY_ADMIN must be set before building lp_handler");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must exist"));
    let output = out_dir.join("security_admin.rs");
    let generated = format!(
        "pub const SECURITY_ADMIN: Pubkey = pubkey!(\"{}\");\n",
        security_admin
    );

    fs::write(output, generated).expect("failed to write generated security admin const");
}
