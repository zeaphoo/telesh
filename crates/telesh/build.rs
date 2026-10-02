fn main() {
    // `agent-ui/dist` is embedded at compile time via rust-embed. Without this,
    // cargo does not notice a rebuilt `dist/` and keeps serving the old UI
    // (stale assets / favicon). Rebuild the UI first: `cd agent-ui && pnpm build`.
    println!("cargo:rerun-if-changed=../../agent-ui/dist");
}
