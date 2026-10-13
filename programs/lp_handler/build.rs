use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=SECURITY_ADMIN");

    let security_admin =
        env::var("SECURITY_ADMIN").unwrap_or_else(|_| "8i1RFM7r2qqL4ojK25zFLVu89fqfrqTbUhgyApNcRrNL".to_string());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must exist"));
    let output = out_dir.join("security_admin.rs");
    let generated = format!(
        "pub const SECURITY_ADMIN: Pubkey = pubkey!(\"{}\");\n",
        security_admin
    );

    fs::write(output, generated).expect("failed to write generated security admin const");
}
