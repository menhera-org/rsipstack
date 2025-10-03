use std::env;

fn main() {
    if env::var("CARGO_CFG_TARGET_ARCH")
        .map(|arch| arch == "wasm32")
        .unwrap_or(false)
    {
        panic!("ftth-rsipstack does not support wasm32 targets");
    }
}
