fn main() {
    // `server-ui/dist` is embedded at compile time via rust-embed. Without this,
    // cargo does not notice a rebuilt `dist/` and keeps serving the old dashboard
    // (stale assets / favicon). Rebuild the UI first: `cd server-ui && pnpm build`.
    println!("cargo:rerun-if-changed=../../server-ui/dist");
}
