use std::env;

fn main() {
    // Windows looks for the DLLs a program imports beside the program first, where someone could
    // plant one for it to run as an administrator. So it imports DLLs from System32 only, and
    // links the VC runtime in, which System32 has only where Visual C++'s is installed. Windows'
    // own C runtime is always there.
    static_vcruntime::metabuild();
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo::rustc-link-arg-bin=cleanse-elevate=/DEPENDENTLOADFLAG:0x800");
        // It only works as an administrator, so its manifest asks Windows for one, and Windows
        // shows the UAC prompt when the desktop app starts it. Only the program asks: its tests
        // don't.
        println!("cargo::rustc-link-arg-bin=cleanse-elevate=/MANIFEST:EMBED");
        println!(
            "cargo::rustc-link-arg-bin=cleanse-elevate=/MANIFESTUAC:level='requireAdministrator'"
        );
    }
}
