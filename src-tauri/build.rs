fn main() {
    // The Google sign-in client is baked in at build time (from CI secrets),
    // so it never has to live in the repository.
    println!("cargo:rerun-if-env-changed=LEO_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=LEO_GOOGLE_CLIENT_SECRET");
    tauri_build::build()
}
