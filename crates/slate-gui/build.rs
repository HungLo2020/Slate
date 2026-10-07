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
    // CMake found Qt (CMAKE_PREFIX_PATH/Qt6_DIR select a specific installation)
    // and lists the libraries to link. pkg-config remains a fallback.
    let listed = std::fs::read_to_string(dst.join("lib/qt-link.txt")).unwrap_or_default();
    let libraries: Vec<&str> = listed.lines().filter(|l| !l.trim().is_empty()).collect();
    if libraries.is_empty() {
        let mut modules = vec![
            "Qt6Quick",
            "Qt6Qml",
            "Qt6Gui",
            "Qt6Widgets",
            "Qt6PrintSupport",
            "Qt6Core",
        ];
        if smoke {
            modules.push("Qt6Test");
        }
        for lib in modules {
            pkg_config::Config::new()
                .atleast_version("6.4")
                .probe(lib)
                .expect("Install Qt 6.4+ development packages");
        }
    } else {
        for library in libraries {
            let path = std::path::Path::new(library.trim());
            if let Some(dir) = path.parent() {
                // Frameworks and import libraries also need their directory.
                println!("cargo:rustc-link-search=native={}", dir.display());
            }
            println!("cargo:rustc-link-arg={}", path.display());
        }
    }
    // The C++ runtime the native adapter was compiled against.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
