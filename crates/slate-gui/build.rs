fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=../../resources/slate.svg");
    let smoke = std::env::var_os("CARGO_FEATURE_SMOKE").is_some();
    let dst = cmake::Config::new("native")
        .define("SLATE_SMOKE_TEST", if smoke { "ON" } else { "OFF" })
        .define("CMAKE_BUILD_TYPE", "Release")
        .build();
    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=slate_qt");
    for lib in ["Qt6Quick", "Qt6Qml", "Qt6Gui", "Qt6Widgets", "Qt6Core"] {
        pkg_config::Config::new()
            .atleast_version("6.4")
            .probe(lib)
            .expect("Install Qt 6.4+ development packages");
    }
    if smoke {
        pkg_config::Config::new()
            .atleast_version("6.4")
            .probe("Qt6Test")
            .expect("Qt Test is needed for GUI smoke tests");
    }
    println!("cargo:rustc-link-lib=stdc++");
}
