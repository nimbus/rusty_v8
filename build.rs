// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.
use fslock::LockFile;
use miniz_oxide::MZFlush;
use miniz_oxide::MZStatus;
use miniz_oxide::StreamResult;
use miniz_oxide::inflate::stream::InflateState;
use miniz_oxide::inflate::stream::inflate;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use which::which;

fn main() {
  println!("cargo:rerun-if-changed=.gn");
  println!("cargo:rerun-if-changed=BUILD.gn");
  println!("cargo:rerun-if-changed=src/binding.cc");

  // These are all the environment variables that we check. This is
  // probably more than what is needed, but missing an important
  // variable can lead to broken links when switching rusty_v8
  // versions.
  let envs = vec![
    "CCACHE",
    "CLANG_BASE_PATH",
    "CXXSTDLIB",
    "DENO_TRYBUILD",
    "DOCS_RS",
    "GN",
    "GN_ARGS",
    "HOST",
    "NINJA",
    "OUT_DIR",
    "RUSTY_V8_ARCHIVE",
    "RUSTY_V8_MIRROR",
    "RUSTY_V8_MUSL_SYSROOT",
    "RUSTY_V8_SRC_BINDING_PATH",
    "RUSTY_V8_VERSION",
    "SCCACHE",
    "V8_FORCE_DEBUG",
    "V8_FROM_SOURCE",
    "PYTHON",
    "DISABLE_CLANG",
    "EXTRA_GN_ARGS",
    "PRINT_GN_ARGS",
    "CARGO_ENCODED_RUSTFLAGS",
  ];
  for env in envs {
    println!("cargo:rerun-if-env-changed={env}");
  }

  // Detect if trybuild tests are being compiled.
  let is_trybuild = env::var_os("DENO_TRYBUILD").is_some();

  // Don't build V8 if "cargo doc" is being run. This is to support docs.rs.
  let is_cargo_doc = env::var_os("DOCS_RS").is_some();

  // Don't build V8 if the rust language server (RLS) is running.
  let is_rls = env::var_os("CARGO")
    .map(PathBuf::from)
    .as_ref()
    .and_then(|p| p.file_stem())
    .and_then(|f| f.to_str())
    .is_some_and(|s| s.starts_with("rls"));

  // Early exit
  if is_cargo_doc || is_rls {
    print_packaged_src_binding_path();
    return;
  }

  print_link_flags();

  // Don't attempt rebuild but link
  if is_trybuild {
    println!(
      "cargo:rustc-env=RUSTY_V8_SRC_BINDING_PATH={}",
      env::var("RUSTY_V8_SRC_BINDING_PATH").unwrap()
    );
    return;
  }

  let is_asan = if let Some(rustflags) = env::var_os("CARGO_ENCODED_RUSTFLAGS")
  {
    let rustflags = rustflags.to_string_lossy();
    rustflags.find("-Z sanitizer=address").is_some()
      || rustflags.find("-Zsanitizer=address").is_some()
  } else {
    false
  };

  // Cargo likes to run multiple build scripts at once sometimes.
  // Nothing that follows is safe to run multiple times at once,
  // because we store everything in a parent directory of OUT_DIR.
  let _lockfile = acquire_lock();

  // Build from source
  if env_bool("V8_FROM_SOURCE") {
    if is_asan && env::var_os("OPT_LEVEL").unwrap_or_default() == "0" {
      panic!(
        "v8 crate cannot be compiled with OPT_LEVEL=0 and ASAN.\nTry `[profile.dev.package.v8] opt-level = 1`.\nAborting before miscompilations cause issues."
      );
    }

    // cargo publish doesn't like pyc files.
    unsafe {
      env::set_var("PYTHONDONTWRITEBYTECODE", "1");
    }

    build_v8(is_asan);
    build_binding();

    return;
  }

  validate_custom_archive_overrides(
    env::var_os("RUSTY_V8_ARCHIVE").is_some(),
    env::var_os("RUSTY_V8_SRC_BINDING_PATH").is_some(),
  )
  .unwrap_or_else(|error| panic!("{error}"));

  print_prebuilt_src_binding_path();

  download_static_lib_binaries();
}

fn validate_custom_archive_overrides(
  has_archive: bool,
  has_binding: bool,
) -> Result<(), &'static str> {
  if has_archive && !has_binding {
    return Err(
      "RUSTY_V8_ARCHIVE requires RUSTY_V8_SRC_BINDING_PATH so the custom V8 \
       library and generated Rust binding stay matched",
    );
  }
  Ok(())
}

fn acquire_lock() -> LockFile {
  let root = env::current_dir().unwrap();
  let out_dir = env::var_os("OUT_DIR").unwrap();
  let lockfilepath = root
    .join(out_dir)
    .parent()
    .unwrap()
    .parent()
    .unwrap()
    .join("v8.fslock");
  let mut lockfile = LockFile::open(&lockfilepath)
    .expect("Couldn't open lib download lockfile.");
  lockfile.lock_with_pid().expect("Couldn't get lock");
  println!("lockfile: {lockfilepath:?}");
  lockfile
}

fn build_binding() {
  // Bindgen needs Clang 21+ for V8's libc++ builtin type traits.
  if env::var("LIBCLANG_PATH").is_err() {
    eprintln!("Warning: LIBCLANG_PATH not set. Bindgen requires Clang 21+.");
    eprintln!("Set LIBCLANG_PATH to your Clang 21 installation:");
    eprintln!("  Linux:  export LIBCLANG_PATH=/usr/lib/llvm-21/lib");
    eprintln!("  macOS:  export LIBCLANG_PATH=$(brew --prefix llvm@21)/lib");
  }

  let output = Command::new(python())
    .arg("./tools/get_bindgen_args.py")
    .arg("--gn-out")
    .arg(build_dir().join("gn_out"))
    .output()
    .unwrap();
  let args = String::from_utf8(output.stdout).unwrap();
  let args = args.split('\0').collect::<Vec<_>>();

  // Filter out V8's custom libc++ and module args from GN, we'll add them back
  // manually with correct ordering for bindgen
  let filtered_args: Vec<&str> = args
    .iter()
    .filter(|arg| {
      !arg.starts_with("-fmodule")
        && !arg.starts_with("-fno-implicit-module")
        && !arg.starts_with("-Xclang")
        && !arg.contains("DUSE_LIBCXX_MODULES")
        && !arg.contains("-nostdinc++")
        && !arg.contains("-isystem")
        && !arg.contains("libc++")
    })
    .copied()
    .collect();

  // Use V8's custom libc++ headers (requires Clang 21+ libclang via LIBCLANG_PATH)
  // IMPORTANT: libc++ headers must come before clang builtins
  let mut clang_args = vec![
    "-x".to_string(),
    "c++".to_string(),
    "-std=c++20".to_string(),
    "-nostdinc++".to_string(),
    "-Iv8/include".to_string(),
    "-I.".to_string(),
    "-isystembuildtools/third_party/libc++".to_string(),
    "-isystemthird_party/libc++/src/include".to_string(),
    "-isystemthird_party/libc++abi/src/include".to_string(),
  ];

  // libclang supplies its own resource include directory. Adding it again on
  // Linux makes builtin stdint.h's include_next re-enter the guarded builtin
  // header instead of reaching the platform C headers.
  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
  if target_os == "macos" {
    let output = Command::new("xcrun")
      .args(["--show-sdk-path"])
      .output()
      .unwrap();
    let sdk_path = String::from_utf8(output.stdout).unwrap();
    clang_args.push("-isysroot".to_string());
    clang_args.push(sdk_path.trim().to_string());
  } else if target_os == "linux" {
    // Parse the V8 headers against the musl sysroot. bindgen already targets
    // the musl triple (from $TARGET), so without this it looks for the target
    // arch's glibc multiarch headers, which aren't installed when cross-
    // compiling (e.g. aarch64 glibc headers on an x86_64 runner).
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "musl"
      && let Ok(sysroot) = env::var("RUSTY_V8_MUSL_SYSROOT")
    {
      clang_args.push(format!("--sysroot={sysroot}"));
    }
  } else if target_os == "ios" {
    // iOS: point bindgen at the iOS (device) or iOS-simulator SDK and set the
    // matching clang target triple so the V8 headers parse correctly.
    let target_triple = env::var("TARGET").unwrap();
    let is_sim = target_triple.ends_with("-sim")
      || target_triple.starts_with("x86_64-apple-ios");
    let sdk = if is_sim {
      "iphonesimulator"
    } else {
      "iphoneos"
    };
    let output = Command::new("xcrun")
      .args(["--sdk", sdk, "--show-sdk-path"])
      .output()
      .unwrap();
    let sdk_path = String::from_utf8(output.stdout).unwrap();
    clang_args.push("-isysroot".to_string());
    clang_args.push(sdk_path.trim().to_string());
    let clang_target = if is_sim {
      "arm64-apple-ios-simulator"
    } else {
      "arm64-apple-ios"
    };
    clang_args.push(format!("--target={clang_target}"));
  }

  let bindings = bindgen::Builder::default()
    .header("src/binding.hpp")
    .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
    .clang_args(clang_args)
    .clang_args(filtered_args)
    .generate_cstr(true)
    .rustified_enum(".*UseCounterFeature")
    .rustified_enum(".*ModuleImportPhase")
    .rustified_enum(".*Intercepted")
    .bitfield_enum(".*GCType")
    .bitfield_enum(".*GCCallbackFlags")
    .allowlist_item("v8__.*")
    .allowlist_item("cppgc__.*")
    .allowlist_item("RustObj")
    .allowlist_item("memory_span_t")
    .allowlist_item("const_memory_span_t")
    .allowlist_item("ExternalConstOneByteStringResource")
    .blocklist_item("cppgc.*Visitor")
    .blocklist_item("RustObj.*Trace")
    .generate()
    .expect("Unable to generate bindings");

  let out_path = build_dir().join("gn_out").join("src_binding.rs");
  println!(
    "cargo:rustc-env=RUSTY_V8_SRC_BINDING_PATH={}",
    out_path.display()
  );
  bindings
    .write_to_file(out_path)
    .expect("Couldn't write bindings!");
}

fn build_v8(is_asan: bool) {
  unsafe {
    env::set_var("DEPOT_TOOLS_WIN_TOOLCHAIN", "0");
  }

  if need_gn_ninja_download() {
    download_ninja_gn_binaries();
  }

  download_rust_toolchain();

  // `#[cfg(...)]` attributes don't work as expected from build.rs -- they refer to the configuration
  // of the host system which the build.rs script will be running on. In short, `cfg!(target_<os/arch>)`
  // is actually the host os/arch instead of target os/arch while cross compiling. Instead, Environment variables
  // are the officially approach to get the target os/arch in build.rs.
  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
  let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
  // On windows, rustc cannot link with a V8 debug build.
  let mut gn_args = if is_debug() && target_os != "windows" {
    // Note: When building for Android aarch64-qemu, use release instead of debug.
    vec!["is_debug=true".to_string()]
  } else {
    vec!["is_debug=false".to_string()]
  };
  if is_asan {
    gn_args.push("is_asan=true".to_string());
  }
  gn_args.push(format!(
    "use_custom_libcxx={}",
    env::var("CARGO_FEATURE_USE_CUSTOM_LIBCXX").is_ok()
  ));

  let extra_args = {
    if env::var("CARGO_FEATURE_V8_ENABLE_SANDBOX").is_ok() {
      vec![
        // Enable pointer compression (along with its dependencies)
        "v8_enable_sandbox=true",
        "v8_enable_external_code_space=true", // Needed for sandbox
        "v8_enable_pointer_compression=true",
        // Note that sandbox requires shared_ro_heap and verify_heap
        // to be true/default
      ]
    } else {
      let mut opts = vec![
        // Disable sandbox
        "v8_enable_sandbox=false",
      ];

      if env::var("CARGO_FEATURE_V8_ENABLE_POINTER_COMPRESSION").is_ok() {
        opts.push("v8_enable_pointer_compression=true");
      } else {
        opts.push("v8_enable_pointer_compression=false");
      }

      opts
    }
  };

  for arg in extra_args {
    gn_args.push(arg.to_string());
  }

  gn_args.push(format!(
    "v8_enable_v8_checks={}",
    env::var("CARGO_FEATURE_V8_ENABLE_V8_CHECKS").is_ok()
  ));

  gn_args.push(format!(
    "rusty_v8_enable_simdutf={}",
    env::var("CARGO_FEATURE_SIMDUTF").is_ok()
  ));

  // Fix GN's host_cpu detection when using x86_64 bins on Apple Silicon
  if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
    gn_args.push("host_cpu=\"arm64\"".to_string());
  }

  if env::var_os("DISABLE_CLANG").is_some() {
    gn_args.push("is_clang=false".into());
    // -gline-tables-only is Clang-only
    gn_args.push("line_tables_only=false".into());
  } else if let Some(clang_base_path) = find_compatible_system_clang() {
    println!("clang_base_path (system): {}", clang_base_path.display());
    gn_args.push(format!("clang_base_path={clang_base_path:?}"));
    gn_args.push("treat_warnings_as_errors=false".to_string());
  } else {
    println!("using Chromium's clang");
    let clang_base_path = clang_download();
    gn_args.push(format!("clang_base_path={clang_base_path:?}"));

    if target_os == "android" && target_arch == "aarch64" {
      gn_args.push("treat_warnings_as_errors=false".to_string());
    }
  }

  if let Some(p) = env::var_os("SCCACHE") {
    cc_wrapper(&mut gn_args, Path::new(&p));
  } else if let Ok(p) = which("sccache") {
    cc_wrapper(&mut gn_args, &p);
  } else if let Some(p) = env::var_os("CCACHE") {
    cc_wrapper(&mut gn_args, Path::new(&p));
  } else if let Ok(p) = which("ccache") {
    cc_wrapper(&mut gn_args, &p);
  } else {
    println!("cargo:warning=Not using sccache or ccache");
  }

  // Forward caller-provided GN args verbatim.
  let gn_args_env = env::var("GN_ARGS").unwrap_or_default();
  if !gn_args_env.trim().is_empty() {
    gn_args.push(gn_args_env.clone());
  }

  // rusty_v8 ships a single static archive that downstream crates may link
  // into a shared library (cdylib). On Linux, V8's default "local-exec" TLS
  // model emits R_X86_64_TPOFF32 relocations against thread-locals such as
  // `g_current_isolate_`, which lld refuses to place in a `-shared` object,
  // so any cdylib that links the archive fails to link. Enabling this V8 GN
  // arg routes the `V8_TLS_USED_IN_LIBRARY` define into both `internal_config`
  // and the `features` config, switching V8 to the shared-library-safe TLS
  // path (local-dynamic model + out-of-line accessor) uniformly across V8's
  // own sources and rusty_v8's bindings.
  if target_os == "linux"
    && !gn_args_env.contains("v8_monolithic_for_shared_library")
  {
    gn_args.push("v8_monolithic_for_shared_library=true".to_string());
  }
  // cross-compilation setup
  if target_arch == "aarch64" {
    gn_args.push(r#"target_cpu="arm64""#.to_string());
    if target_os == "linux" {
      gn_args.push("use_sysroot=true".to_string());
      maybe_install_sysroot("arm64");
      maybe_install_sysroot("amd64");
    }
  }
  if target_arch == "arm" {
    gn_args.push(r#"target_cpu="arm""#.to_string());
    gn_args.push(r#"v8_target_cpu="arm""#.to_string());
    gn_args.push("use_sysroot=true".to_string());
    maybe_install_sysroot("i386");
    maybe_install_sysroot("arm");
  }
  if target_arch == "riscv64" {
    gn_args.push(r#"target_cpu="riscv64""#.to_string());
    // Cross compiling needs to set v8_target_cpu
    gn_args.push(r#"v8_target_cpu="riscv64""#.to_string());
    if target_os == "linux" {
      gn_args.push("use_sysroot=true".to_string());
      maybe_install_sysroot("riscv64");
      maybe_install_sysroot("amd64");
    }
  }

  // musl libc. V8's build targets glibc by default; the vendored build config
  // grows a target-scoped `use_musl` arg (see //build/config/rust.gni,
  // sysroot.gni, c++/BUILD.gn, toolchain/*). The final librusty_v8.a is a
  // static archive of musl-compiled objects (never linked here), so the target
  // toolchain needs only musl headers -- the executable build tools (torque,
  // mksnapshot, code generators) are built with a separate glibc toolchain so
  // they link and run on the (glibc) build host.
  let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
  if target_env == "musl" && target_os == "linux" {
    gn_args.push("use_musl=true".to_string());
    // V8-as-a-library has no glib dependency; skip it so a musl target_sysroot
    // doesn't send pkg-config looking for glib inside the sysroot.
    gn_args.push("use_glib=false".to_string());
    // Build libstd + V8's internal Rust crates from source for the musl triple;
    // V8's vendored Rust toolchain only ships a glibc host std.
    gn_args.push("rust_prebuilt_stdlib=false".to_string());
    // Some V8 sources have glibc-only code paths (e.g. execinfo-based
    // backtraces in stack_trace_posix.cc) whose helpers are unused on musl,
    // tripping -Werror,-Wunused-const-variable. Like the iOS/Android cross
    // builds, don't treat warnings as errors here.
    gn_args.push("treat_warnings_as_errors=false".to_string());

    match target_arch.as_str() {
      "x86_64" => {
        // Host cpu == target cpu, so V8 would build the executable build tools
        // with the (musl) default toolchain. Force the host and snapshot
        // toolchains to a dedicated glibc toolchain so those tools stay glibc.
        let glibc = "//build/toolchain/linux:clang_x64_glibc";
        gn_args.push(format!("host_toolchain=\"{glibc}\""));
        gn_args.push(format!("v8_snapshot_toolchain=\"{glibc}\""));
        // That glibc toolchain builds against the amd64 sysroot, same as the
        // host side of the aarch64/riscv64 cross builds.
        maybe_install_sysroot("amd64");
      }
      "aarch64" => {
        // Cross build (x64 host -> arm64 target). The host (clang_x64) and
        // snapshot (clang_x64_v8_arm64) toolchains are already non-default, so
        // the target-scoped `use_musl` guard keeps them glibc automatically --
        // no toolchain overrides needed. target_cpu and the amd64/arm64
        // sysroots are set by the aarch64 cross-compilation block above.
      }
      other => panic!(
        "musl builds are only supported for x86_64 and aarch64 (got {other})"
      ),
    }

    // Cross-compiling on a glibc host needs a musl sysroot for the target's
    // headers/libs. Native musl builds (e.g. on Alpine) can leave this unset.
    if let Ok(sysroot) = env::var("RUSTY_V8_MUSL_SYSROOT") {
      gn_args.push(format!("target_sysroot=\"{sysroot}\""));
    }
  }

  let target_triple = env::var("TARGET").unwrap();
  // check if the target triple describes a non-native environment
  if target_triple != env::var("HOST").unwrap() && target_os == "android" {
    let arch = if target_arch == "x86_64" {
      "x64"
    } else if target_arch == "aarch64" {
      "arm64"
    } else {
      "unknown"
    };
    if target_arch == "x86_64" {
      maybe_install_sysroot("amd64");
    }
    gn_args.push(format!(r#"v8_target_cpu="{arch}""#).to_string());
    gn_args.push(format!(r#"target_cpu="{arch}""#).to_string());
    gn_args.push(r#"target_os="android""#.to_string());
    gn_args.push("treat_warnings_as_errors=false".to_string());
    gn_args.push("use_sysroot=true".to_string());

    // NDK 23 and above removes libgcc entirely.
    // https://github.com/rust-lang/rust/pull/85806
    if !Path::new("./third_party/android_ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang++").exists() {
        assert!(Command::new("curl")
        .arg("-L")
        .arg("-o").arg("./third_party/android-ndk-r26c-linux.zip")
        .arg("https://dl.google.com/android/repository/android-ndk-r26c-linux.zip")
        .status()
        .unwrap()
        .success());

        assert!(Command::new("unzip")
        .arg("-d").arg("./third_party/")
        .arg("-o")
        .arg("-q")
        .arg("./third_party/android-ndk-r26c-linux.zip")
        .status()
        .unwrap()
        .success());

        fs::rename("./third_party/android-ndk-r26c", "./third_party/android_ndk").unwrap();
        fs::remove_file("./third_party/android-ndk-r26c-linux.zip").unwrap();
      }
    static CHROMIUM_URI: &str = "https://chromium.googlesource.com";
    maybe_clone_repo(
      "./third_party/android_platform",
      &format!("{CHROMIUM_URI}/chromium/src/third_party/android_platform.git",),
    );
    maybe_clone_repo(
      "./third_party/catapult",
      &format!("{CHROMIUM_URI}/catapult.git"),
    );
  }

  // iOS / iOS-simulator. iOS denies the JIT entitlement to non-WebKit apps, so
  // a device build must be jitless -- which in turn requires V8's optimizing
  // tiers (Sparkplug/Maglev/Turbofan) and WebAssembly to be disabled. The
  // simulator runs on the host and could keep the JIT, but WebAssembly is
  // disabled there too because Torque can't generate the Wasm builtins in this
  // configuration. `target_cpu="arm64"` is already set above for aarch64.
  // Pass an explicit `target_os="ios"` in GN_ARGS to fully override this.
  if target_os == "ios" && !gn_args_env.contains(r#"target_os="ios""#) {
    let is_sim = target_triple.ends_with("-sim")
      || target_triple.starts_with("x86_64-apple-ios");
    gn_args.push(r#"target_os="ios""#.to_string());
    gn_args.push(format!(
      r#"target_environment="{}""#,
      if is_sim { "simulator" } else { "device" }
    ));
    gn_args.push(r#"ios_deployment_target="14.0""#.to_string());
    gn_args.push("ios_enable_code_signing=false".to_string());
    gn_args.push("treat_warnings_as_errors=false".to_string());
    gn_args.push("v8_enable_webassembly=false".to_string());
    if !is_sim {
      // Device: no JIT permitted -> jitless build, all tiers off.
      gn_args.push("v8_jitless=true".to_string());
      gn_args.push("v8_enable_sparkplug=false".to_string());
      gn_args.push("v8_enable_maglev=false".to_string());
      gn_args.push("v8_enable_turbofan=false".to_string());
    }
  }

  if target_triple.starts_with("i686-") {
    gn_args.push(r#"target_cpu="x86""#.to_string());
  }

  let gn_out = run_gn_gen(&gn_args);
  assert!(gn_out.exists());
  assert!(gn_out.join("args.gn").exists());
  if env_bool("PRINT_GN_ARGS") {
    print_gn_args(&gn_out);
  }
  build("rusty_v8", None);
}

fn print_gn_args(gn_out_dir: &Path) {
  assert!(
    Command::new(gn())
      .arg(format!("--script-executable={}", python()))
      .arg("args")
      .arg(gn_out_dir)
      .arg("--list")
      .status()
      .unwrap()
      .success()
  );
}

fn maybe_clone_repo(dest: &str, repo: &str) {
  if !Path::new(&dest).exists() {
    assert!(
      Command::new("git")
        .arg("clone")
        .arg("--depth=1")
        .arg(repo)
        .arg(dest)
        .status()
        .unwrap()
        .success()
    );
  }
}

fn maybe_install_sysroot(arch: &str) {
  let sysroot_path = format!("build/linux/debian_sid_{arch}-sysroot");
  if !PathBuf::from(sysroot_path).is_dir() {
    assert!(
      Command::new(python())
        .arg("./build/linux/sysroot_scripts/install-sysroot.py")
        .arg(format!("--arch={arch}"))
        .status()
        .unwrap()
        .success()
    );
  }
}

fn download_ninja_gn_binaries() {
  let target_dir = build_dir().join("ninja_gn_binaries");

  let gn = target_dir.join("gn").join("gn");
  let ninja = target_dir.join("ninja").join("ninja");
  #[cfg(windows)]
  let gn = gn.with_extension("exe");
  #[cfg(windows)]
  let ninja = ninja.with_extension("exe");

  if !gn.exists() || !ninja.exists() {
    assert!(
      Command::new(python())
        .arg("./tools/ninja_gn_binaries.py")
        .arg("--dir")
        .arg(&target_dir)
        .status()
        .unwrap()
        .success()
    );
  }
  assert!(gn.exists());
  assert!(ninja.exists());
  unsafe {
    env::set_var("GN", gn);
  }
  if env::var("NINJA").is_err() {
    unsafe {
      env::set_var("NINJA", ninja);
    }
  }
}

fn download_rust_toolchain() {
  assert!(
    Command::new(python())
      .arg("./tools/rust_toolchain.py")
      .status()
      .unwrap()
      .success()
  );
}

fn prebuilt_profile() -> &'static str {
  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
  // Use v8 in release mode unless $V8_FORCE_DEBUG=true
  // Note: we always use the release build on windows.
  if target_os != "windows" && env_bool("V8_FORCE_DEBUG") {
    "debug"
  } else {
    "release"
  }
}

fn prebuilt_features_suffix() -> String {
  prebuilt_features_suffix_from_flags(
    env::var("CARGO_FEATURE_V8_ENABLE_POINTER_COMPRESSION").is_ok(),
    env::var("CARGO_FEATURE_V8_ENABLE_SANDBOX").is_ok(),
    env::var("CARGO_FEATURE_SIMDUTF").is_ok(),
  )
}

fn prebuilt_features_suffix_from_flags(
  pointer_compression: bool,
  sandbox: bool,
  simdutf: bool,
) -> String {
  let mut features = String::new();
  if pointer_compression {
    features.push_str("_ptrcomp");
  }
  if sandbox {
    features.push_str("_sandbox");
  }
  if simdutf {
    features.push_str("_simdutf");
  }
  features
}

fn static_lib_name(suffix: &str) -> String {
  static_lib_name_for_target(&env::var("TARGET").unwrap(), suffix)
}

fn static_lib_name_for_target(target: &str, suffix: &str) -> String {
  if target.contains("windows") {
    format!("rusty_v8{suffix}.lib")
  } else {
    format!("librusty_v8{suffix}.a")
  }
}

fn validate_prebuilt_configuration(
  target: &str,
  profile: &str,
  features: &str,
) -> Result<(), String> {
  let requested = if features.is_empty() {
    "default"
  } else {
    features.trim_start_matches('_')
  };
  let published = include_str!("tools/nimbus_release_targets.tsv")
    .lines()
    .filter(|line| !line.is_empty() && !line.starts_with('#'))
    .filter_map(|line| {
      let mut fields = line.split('\t');
      let _os = fields.next()?;
      let manifest_target = fields.next()?;
      let _pointer_compression = fields.next()?;
      let _build_only = fields.next()?;
      let suffixes = fields.next()?;
      (fields.next().is_none() && manifest_target == target).then_some(suffixes)
    })
    .any(|suffixes| suffixes.split(',').any(|suffix| suffix == requested));
  if profile != "release" || !published {
    return Err(format!(
      "Nimbus does not publish a rusty_v8 prebuilt for target {target} with \
       profile {profile} and feature configuration {requested}; set \
       V8_FROM_SOURCE=1 or provide both RUSTY_V8_ARCHIVE and \
       RUSTY_V8_SRC_BINDING_PATH"
    ));
  }
  Ok(())
}

fn prebuilt_archive_name(
  target: &str,
  profile: &str,
  features: &str,
) -> Result<String, String> {
  validate_prebuilt_configuration(target, profile, features)?;
  Ok(format!(
    "{}.gz",
    static_lib_name_for_target(
      target,
      &format!("{features}_{profile}_{target}"),
    )
  ))
}

fn prebuilt_binding_name(
  target: &str,
  profile: &str,
  features: &str,
) -> Result<String, String> {
  validate_prebuilt_configuration(target, profile, features)?;
  Ok(src_binding_name(target, profile, features))
}

fn src_binding_name(target: &str, profile: &str, features: &str) -> String {
  format!("src_binding{features}_{profile}_{target}.rs")
}

fn prebuilt_version() -> String {
  resolve_prebuilt_version(
    &env::var("CARGO_PKG_VERSION").unwrap(),
    env::var("RUSTY_V8_VERSION").ok(),
  )
}

fn resolve_prebuilt_version(
  package_version: &str,
  override_version: Option<String>,
) -> String {
  override_version.unwrap_or_else(|| format!("{package_version}-nimbus.1"))
}

const DEFAULT_NIMBUS_PREBUILT_BASE: &str =
  "https://github.com/nimbus/rusty_v8/releases/download";

fn prebuilt_base() -> String {
  env::var("RUSTY_V8_MIRROR")
    .unwrap_or_else(|_| DEFAULT_NIMBUS_PREBUILT_BASE.into())
}

fn prebuilt_asset_url(base: &str, version: &str, name: &str) -> String {
  format!("{base}/v{version}/{name}")
}

fn static_lib_url() -> String {
  if let Ok(custom_archive) = env::var("RUSTY_V8_ARCHIVE") {
    return custom_archive;
  }
  let base = prebuilt_base();
  let version = prebuilt_version();
  let target = env::var("TARGET").unwrap();
  let profile = prebuilt_profile();
  let features = prebuilt_features_suffix();
  let archive = prebuilt_archive_name(&target, profile, &features)
    .unwrap_or_else(|error| panic!("{error}"));
  prebuilt_asset_url(&base, &version, &archive)
}

fn static_lib_path() -> PathBuf {
  static_lib_dir().join(static_lib_name(""))
}

fn static_checksum_path(path: &Path) -> PathBuf {
  let mut path = path.to_path_buf();
  path.set_extension("sum");
  path
}

fn static_lib_dir() -> PathBuf {
  build_dir().join("gn_out").join("obj")
}

fn build_dir() -> PathBuf {
  let cwd = env::current_dir().unwrap();

  // target/debug//build/rusty_v8-d9e5a424d4f96994/out/
  let out_dir = env::var_os("OUT_DIR").expect(
    "The 'OUT_DIR' environment is not set (it should be something like \
     'target/debug/rusty_v8-{hash}').",
  );
  let out_dir_abs = cwd.join(out_dir);

  // This would be `target/debug` or `target/release`
  out_dir_abs
    .parent()
    .unwrap()
    .parent()
    .unwrap()
    .parent()
    .unwrap()
    .to_path_buf()
}

fn replace_non_alphanumeric(url: &str) -> String {
  url
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
    .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChecksumPolicy {
  Required,
  TrustedCustomArchive,
}

fn download_file(url: &str, filename: &Path, checksum: ChecksumPolicy) {
  if cached_download_is_reusable(url, filename, checksum) {
    return;
  }

  let asset_tmp = filename.with_extension("asset.tmp");
  let sidecar_tmp = filename.with_extension("sha256.tmp");
  for path in [&asset_tmp, &sidecar_tmp] {
    if path.exists() {
      println!("Deleting old tmpfile {}", path.display());
      fs::remove_file(path).unwrap();
    }
  }

  download_to_path(url, &asset_tmp);
  let remote_digest = match checksum {
    ChecksumPolicy::Required => {
      let sidecar_url = format!("{url}.sha256");
      download_checksum_sidecar(&sidecar_url, &sidecar_tmp);
      let expected_digest = parse_sha256_sidecar(
        &fs::read_to_string(&sidecar_tmp).unwrap(),
        asset_name_from_url(url),
      )
      .unwrap_or_else(|error| {
        panic!("invalid checksum sidecar {sidecar_url}: {error}")
      });
      let actual_digest = sha256_file(&asset_tmp).unwrap();
      assert_eq!(
        actual_digest, expected_digest,
        "SHA-256 mismatch for V8 prebuilt asset {url}"
      );
      expected_digest
    }
    ChecksumPolicy::TrustedCustomArchive => "trusted-custom-archive".into(),
  };

  copy_archive(&asset_tmp.to_string_lossy(), filename);
  let local_digest = sha256_file(filename).unwrap();
  fs::write(
    static_checksum_path(filename),
    format!("{url}\n{remote_digest}\n{local_digest}\n"),
  )
  .unwrap();
  fs::remove_file(&asset_tmp).unwrap();
  if sidecar_tmp.exists() {
    fs::remove_file(&sidecar_tmp).unwrap();
  }

  assert!(filename.exists());
  assert!(static_checksum_path(filename).exists());
  assert!(!asset_tmp.exists());
  assert!(!sidecar_tmp.exists());
}

fn cached_download_is_reusable(
  url: &str,
  filename: &Path,
  checksum: ChecksumPolicy,
) -> bool {
  // Caller-trusted local paths are mutable build inputs, so always recopy
  // them. Remote custom archives retain the historical URL-keyed cache.
  (checksum == ChecksumPolicy::Required || is_http_url(url))
    && cached_download_matches(url, filename, checksum)
}

fn cached_download_matches(
  url: &str,
  filename: &Path,
  checksum: ChecksumPolicy,
) -> bool {
  let Ok(record) = fs::read_to_string(static_checksum_path(filename)) else {
    return false;
  };
  let mut lines = record.lines();
  let Some(recorded_url) = lines.next() else {
    return false;
  };
  let Some(remote_digest) = lines.next() else {
    return false;
  };
  let Some(local_digest) = lines.next() else {
    return false;
  };
  let remote_digest_matches_policy = match checksum {
    ChecksumPolicy::Required => is_sha256_digest(remote_digest),
    ChecksumPolicy::TrustedCustomArchive => {
      remote_digest == "trusted-custom-archive"
    }
  };
  if recorded_url != url
    || !remote_digest_matches_policy
    || !is_sha256_digest(local_digest)
    || lines.next().is_some()
    || !filename.is_file()
  {
    return false;
  }
  sha256_file(filename).is_ok_and(|digest| digest == local_digest)
}

fn asset_name_from_url(url: &str) -> &str {
  let path = url.split(['?', '#']).next().unwrap();
  path
    .rsplit(['/', '\\'])
    .next()
    .filter(|name| !name.is_empty())
    .unwrap()
}

fn parse_sha256_sidecar(
  contents: &str,
  expected_asset_name: &str,
) -> Result<String, String> {
  let mut fields = contents.split_whitespace();
  let digest = fields.next().ok_or("missing digest")?;
  let asset_name = fields.next().ok_or("missing asset name")?;
  if fields.next().is_some() {
    return Err("unexpected trailing fields".into());
  }
  if !is_sha256_digest(digest) {
    return Err("digest must contain exactly 64 hexadecimal digits".into());
  }
  if asset_name != expected_asset_name {
    return Err(format!(
      "sidecar names {asset_name}, expected {expected_asset_name}"
    ));
  }
  Ok(digest.to_ascii_lowercase())
}

fn is_sha256_digest(value: &str) -> bool {
  value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_file(path: &Path) -> io::Result<String> {
  let mut file = fs::File::open(path)?;
  let mut hasher = Sha256::new();
  let mut buffer = [0_u8; 64 * 1024];
  loop {
    let read = file.read(&mut buffer)?;
    if read == 0 {
      break;
    }
    hasher.update(&buffer[..read]);
  }
  Ok(format!("{:x}", hasher.finalize()))
}

fn download_to_path(url: &str, destination: &Path) {
  if !is_http_url(url) {
    copy_file_contents(Path::new(url), destination);
    return;
  }

  // If there is a `.cargo/.rusty_v8/<escaped URL>` file, use that instead
  // of downloading.
  if let Ok(mut path) = home::cargo_home() {
    path = path.join(".rusty_v8").join(replace_non_alphanumeric(url));
    println!("Looking for download in '{path:?}'");
    if path.exists() {
      copy_file_contents(&path, destination);
      return;
    }
  }

  // Try downloading with deno first, then python, then curl.
  println!("Downloading {url}");
  let status = which("deno").ok().and_then(|deno| {
    println!("Trying with Deno...");
    Command::new(deno)
      .arg("eval")
      .arg(
        "const [url, path] = Deno.args; \
         const resp = await fetch(url); \
         if (!resp.ok) Deno.exit(1); \
         const file = await Deno.open(path, { write: true, create: true }); \
         await resp.body.pipeTo(file.writable);",
      )
      // Note: `deno eval` runs with all permissions implicitly granted and does
      // not accept `--allow-*` flags, so passing them here makes `deno eval`
      // error out ("unexpected argument '--allow-net'") and the download
      // silently falls back to Python/curl.
      .arg("--")
      .arg(url)
      .arg(destination)
      .status()
      .ok()
      .filter(|s| s.success())
  });

  // Try downloading with python. Python is a V8 build dependency,
  // so this saves us from adding a Rust HTTP client dependency.
  let status = match status {
    Some(status) => status,
    _ => {
      println!("Trying with Python...");
      let python_status = Command::new(python())
        .arg("./tools/download_file.py")
        .arg("--url")
        .arg(url)
        .arg("--filename")
        .arg(destination)
        .status();

      // Python is only a required dependency for `V8_FROM_SOURCE` builds.
      // If python is not available, try falling back to curl.
      match python_status {
        Ok(status) if status.success() => status,
        _ => {
          println!("Python downloader failed, trying with curl.");
          Command::new("curl")
            .arg("-L")
            .arg("-f")
            .arg("--silent")
            .arg("--show-error")
            .arg("-o")
            .arg(destination)
            .arg(url)
            .status()
            .unwrap()
        }
      }
    }
  };

  if !status.success() {
    panic!(
      "Failed to download V8 prebuilt asset from {url}\n\
       This is usually because no prebuilt is published for your target, \
       in which case you should compile V8 from source by setting V8_FROM_SOURCE=1. \
       It can also indicate a network connectivity problem."
    );
  }
  assert!(destination.exists());
}

fn download_checksum_sidecar(url: &str, destination: &Path) {
  if !is_http_url(url) && !Path::new(url).is_file() {
    panic!(
      "Nimbus V8 file mirror is missing the required checksum sidecar at \
       {url}; mirror each release asset together with its .sha256 file"
    );
  }
  download_to_path(url, destination);
}

fn is_http_url(url: &str) -> bool {
  url.starts_with("http:") || url.starts_with("https:")
}

fn copy_file_contents(source: &Path, destination: &Path) {
  let mut source_file = fs::File::open(source).unwrap_or_else(|error| {
    panic!("failed to read {}: {error}", source.display())
  });
  let mut destination_file =
    fs::File::create(destination).unwrap_or_else(|error| {
      panic!("failed to create {}: {error}", destination.display())
    });
  io::copy(&mut source_file, &mut destination_file).unwrap_or_else(|error| {
    panic!(
      "failed to copy {} to {}: {error}",
      source.display(),
      destination.display()
    )
  });
}

fn download_static_lib_binaries() {
  let url = static_lib_url();
  println!("static lib URL: {url}");

  let dir = static_lib_dir();
  fs::create_dir_all(&dir).unwrap();
  println!("cargo:rustc-link-search={}", dir.display());

  let checksum = if env::var_os("RUSTY_V8_ARCHIVE").is_some() {
    ChecksumPolicy::TrustedCustomArchive
  } else {
    ChecksumPolicy::Required
  };
  download_file(&url, &static_lib_path(), checksum);
}

fn decompress_to_writer<R, W>(input: &mut R, output: &mut W) -> io::Result<()>
where
  R: Read,
  W: Write,
{
  let mut inflate_state = InflateState::default();
  let mut input_buffer = [0; 16 * 1024];
  let mut output_buffer = [0; 16 * 1024];
  let mut input_offset = 0;

  // Skip the gzip header
  gzip_header::read_gz_header(input)?;

  loop {
    let bytes_read = input.read(&mut input_buffer[input_offset..])?;
    let bytes_avail = input_offset + bytes_read;

    let StreamResult {
      bytes_consumed,
      bytes_written,
      status,
    } = inflate(
      &mut inflate_state,
      &input_buffer[..bytes_avail],
      &mut output_buffer,
      MZFlush::None,
    );

    if status != Ok(MZStatus::Ok) && status != Ok(MZStatus::StreamEnd) {
      return Err(io::Error::other(format!("Decompression error {status:?}")));
    }

    output.write_all(&output_buffer[..bytes_written])?;

    // Move remaining bytes to the beginning of the buffer
    input_buffer.copy_within(bytes_consumed..bytes_avail, 0);
    input_offset = bytes_avail - bytes_consumed;

    if status == Ok(MZStatus::StreamEnd) {
      break; // End of decompression
    }
  }

  Ok(())
}

/// Copy the V8 archive at `url` to `filename`.
///
/// This function doesn't use [`fs::copy`] because that would
/// preserve the file attributes such as ownership and mode flags.
/// Instead, it copies the file contents to a new file.
/// This is necessary because the V8 archive could live inside a read-only
/// filesystem, and subsequent builds would fail to overwrite it.
fn copy_archive(url: &str, filename: &Path) {
  println!("Copying {url} to {filename:?}");
  let mut src = fs::File::open(url).unwrap();
  let mut dst = fs::File::create(filename).unwrap();

  // Allow both GZIP and non-GZIP downloads
  let mut header = [0; 2];
  src.read_exact(&mut header).unwrap();
  src.seek(io::SeekFrom::Start(0)).unwrap();
  if header == [0x1f, 0x8b] {
    println!("Detected GZIP archive: {url}");
    decompress_to_writer(&mut src, &mut dst).unwrap();
  } else {
    println!("Not a GZIP archive: {url}");
    io::copy(&mut src, &mut dst).unwrap();
  }
}

fn print_link_flags() {
  println!("cargo:rustc-link-lib=static=rusty_v8");
  let should_dyn_link_libcxx = env::var("CARGO_FEATURE_USE_CUSTOM_LIBCXX")
    .is_err()
    || env::var("GN_ARGS").is_ok_and(|gn_args| {
      gn_args
        .split_whitespace()
        .any(|ba| ba == "use_custom_libcxx=false")
    });

  if should_dyn_link_libcxx {
    // Based on https://github.com/alexcrichton/cc-rs/blob/fba7feded71ee4f63cfe885673ead6d7b4f2f454/src/lib.rs#L2462
    if let Ok(stdlib) = env::var("CXXSTDLIB") {
      if !stdlib.is_empty() {
        println!("cargo:rustc-link-lib=dylib={stdlib}");
      }
    } else {
      let target = env::var("TARGET").unwrap();
      if target.contains("msvc") {
        // nothing to link to
      } else if target.contains("apple")
        || target.contains("freebsd")
        || target.contains("openbsd")
      {
        println!("cargo:rustc-link-lib=dylib=c++");
      } else if target.contains("android") {
        println!("cargo:rustc-link-lib=dylib=c++_shared");
      } else {
        println!("cargo:rustc-link-lib=dylib=stdc++");
      }
    }
  }
  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
  let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();

  if target_os == "windows" {
    println!("cargo:rustc-link-lib=dylib=winmm");
    println!("cargo:rustc-link-lib=dylib=dbghelp");
  }

  if target_env == "msvc" {
    // On Windows, including libcpmt[d]/msvcprt[d] explicitly links the C++
    // standard library, which libc++ needs for exception_ptr internals.
    let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
      .unwrap_or_default()
      .contains("crt-static");
    if crt_static {
      println!("cargo:rustc-link-lib=libcpmt");
    } else {
      println!("cargo:rustc-link-lib=dylib=msvcprt");
    }
  }
}

fn print_prebuilt_src_binding_path() {
  if let Ok(binding) = env::var("RUSTY_V8_SRC_BINDING_PATH") {
    println!("cargo:rustc-env=RUSTY_V8_SRC_BINDING_PATH={binding}");
    return;
  }

  let target = env::var("TARGET").unwrap();
  let profile = prebuilt_profile();
  let features = prebuilt_features_suffix();
  let name = prebuilt_binding_name(&target, profile, &features)
    .unwrap_or_else(|error| panic!("{error}"));

  let src_binding_path = downloaded_binding_path(
    &PathBuf::from(env::var_os("OUT_DIR").unwrap()),
    &name,
  );

  let base = prebuilt_base();
  let version = prebuilt_version();
  let url = prebuilt_asset_url(&base, &version, &name);
  download_file(&url, &src_binding_path, ChecksumPolicy::Required);

  println!(
    "cargo:rustc-env=RUSTY_V8_SRC_BINDING_PATH={}",
    src_binding_path.display()
  );
}

fn print_packaged_src_binding_path() {
  if let Ok(binding) = env::var("RUSTY_V8_SRC_BINDING_PATH") {
    println!("cargo:rustc-env=RUSTY_V8_SRC_BINDING_PATH={binding}");
    return;
  }

  let target = env::var("TARGET").unwrap();
  let profile = prebuilt_profile();
  let features = prebuilt_features_suffix();
  let name = src_binding_name(&target, profile, &features);
  let src_binding_path = packaged_binding_path(&get_dirs().root, &name);

  println!(
    "cargo:rustc-env=RUSTY_V8_SRC_BINDING_PATH={}",
    src_binding_path.display()
  );
}

fn downloaded_binding_path(out_dir: &Path, name: &str) -> PathBuf {
  out_dir.join(name)
}

fn packaged_binding_path(root: &Path, name: &str) -> PathBuf {
  root.join("gen").join(name)
}

// Chromium depot_tools contains helpers
// which delegate to the "relevant" `buildtools`
// directory when invoked, so they don't count.
#[allow(clippy::needless_pass_by_value)]
fn not_in_depot_tools(p: PathBuf) -> bool {
  !p.to_str().unwrap().contains("depot_tools")
}

fn need_gn_ninja_download() -> bool {
  let has_ninja = which("ninja").is_ok_and(not_in_depot_tools)
    || env::var_os("NINJA").is_some();
  let has_gn =
    which("gn").is_ok_and(not_in_depot_tools) || env::var_os("GN").is_some();

  !has_ninja || !has_gn
}

// Chromiums gn arg clang_base_path is currently compatible with:
// * Apples clang and clang from homebrew's llvm@x packages
// * the official binaries from releases.llvm.org
// * unversioned (Linux) packages of clang (if recent enough)
// but unfortunately it doesn't work with version-suffixed packages commonly
// found in Linux packet managers
fn is_compatible_clang_version(clang_path: &Path) -> bool {
  if let Ok(o) = Command::new(clang_path).arg("--version").output() {
    let _output = String::from_utf8(o.stdout).unwrap();
    // TODO check version output to make sure it's supported.
    const _MIN_APPLE_CLANG_VER: f32 = 11.0;
    const _MIN_LLVM_CLANG_VER: f32 = 8.0;
    return true;
  }
  false
}

fn find_compatible_system_clang() -> Option<PathBuf> {
  if let Ok(p) = env::var("CLANG_BASE_PATH") {
    let base_path = Path::new(&p);
    let clang_path = base_path.join("bin").join("clang");
    if is_compatible_clang_version(&clang_path) {
      return Some(base_path.to_path_buf());
    }
  }

  None
}

// Download chromium's clang into OUT_DIR because Cargo will not allow us to
// modify the source directory.
fn clang_download() -> PathBuf {
  let clang_base_path = build_dir().join("clang");
  println!("clang_base_path (downloaded) {}", clang_base_path.display());
  assert!(
    Command::new(python())
      .arg("./tools/clang/scripts/update.py")
      .arg("--output-dir")
      .arg(&clang_base_path)
      .status()
      .unwrap()
      .success()
  );
  assert!(clang_base_path.exists());
  clang_base_path
}

fn cc_wrapper(gn_args: &mut Vec<String>, sccache_path: &Path) {
  gn_args.push(format!("cc_wrapper={sccache_path:?}"));
}

struct Dirs {
  pub out: PathBuf,
  pub root: PathBuf,
}

fn get_dirs() -> Dirs {
  // The OUT_DIR is going to be a crate-specific directory like
  // "target/debug/build/cargo_gn_example-eee5160084460b2c"
  // But we want to share the GN build amongst all crates
  // and return the path "target/debug". So to find it, we walk up three
  // directories.
  // TODO(ry) This is quite brittle - if Cargo changes the directory structure
  // this could break.
  let out = env::var("OUT_DIR").map(PathBuf::from).unwrap();
  let out = out
    .parent()
    .unwrap()
    .parent()
    .unwrap()
    .parent()
    .unwrap()
    .to_owned();

  let root = env::var("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap();
  let mut dirs = Dirs { out, root };
  maybe_symlink_root_dir(&mut dirs);
  dirs
}

#[cfg(not(target_os = "windows"))]
fn maybe_symlink_root_dir(_: &mut Dirs) {}

#[cfg(target_os = "windows")]
fn maybe_symlink_root_dir(dirs: &mut Dirs) {
  // GN produces invalid paths if the source (a.k.a. root) directory is on a
  // different drive than the output. If this is the case we'll create a
  // symlink called 'gn_root' in the out directory, next to 'gn_out', so it
  // appears as if they're both on the same drive.
  use fs::{remove_dir_all, remove_file};
  use std::os::windows::fs::symlink_dir;

  let get_prefix = |p: &Path| {
    p.components()
      .find_map(|c| match c {
        std::path::Component::Prefix(p) => Some(p),
        _ => None,
      })
      .map(|p| p.as_os_str().to_owned())
  };

  let Dirs { out, root } = dirs;
  if get_prefix(out) != get_prefix(root) {
    let symlink = &*out.join("gn_root");
    let target = &*root.canonicalize().unwrap();

    println!("Creating symlink {symlink:?} to {root:?}");

    let mut retries = 0;
    loop {
      match symlink.canonicalize() {
        Ok(existing) if existing == target => break,
        Ok(_) => remove_dir_all(symlink).expect("remove_dir_all failed"),
        Err(err) => {
          println!("symlink.canonicalize failed: {err:?}");
          // we're having very strange issues on GHA when the cache
          // is restored, so trying this out temporarily
          if let Err(err) = remove_dir_all(symlink) {
            eprintln!("remove_dir_all failed: {err:?}");
            if let Err(err) = remove_file(symlink) {
              eprintln!("remove_file failed: {err:?}");
            }
          }
          match symlink_dir(target, symlink) {
            Ok(_) => break,
            Err(err) => {
              println!("symlink_dir failed: {err:?}");
              retries += 1;
              std::thread::sleep(std::time::Duration::from_millis(
                50 * retries,
              ));
              if retries > 4 {
                panic!("Failed to create symlink");
              }
            }
          }
        }
      }
    }

    dirs.root = symlink.to_path_buf();
  }
}

pub fn is_debug() -> bool {
  // Cargo sets PROFILE to either "debug" or "release", which conveniently
  // matches the build modes we support.
  let m = env::var("PROFILE").unwrap();
  if m == "release" {
    false
  } else if m == "debug" {
    true
  } else {
    panic!("unhandled PROFILE value {m}")
  }
}

fn gn() -> String {
  env::var("GN").unwrap_or_else(|_| "gn".to_owned())
}

/*
 * Get the system's python binary - specified via the PYTHON environment
 * variable or defaulting to `python3`.
 */
fn python() -> String {
  env::var("PYTHON").unwrap_or_else(|_| "python3".to_owned())
}

type NinjaEnv = Vec<(String, String)>;

fn ninja(gn_out_dir: &Path, maybe_env: Option<NinjaEnv>) -> Command {
  let cmd_string = env::var("NINJA").unwrap_or_else(|_| "ninja".to_owned());
  let mut cmd = Command::new(&cmd_string);
  cmd.arg("-C");
  cmd.arg(gn_out_dir);
  if !cmd_string.ends_with("autoninja")
    && let Ok(jobs) = env::var("NUM_JOBS")
  {
    cmd.arg("-j");
    cmd.arg(jobs);
  }
  if let Some(env) = maybe_env {
    for item in env {
      cmd.env(item.0, item.1);
    }
  }
  cmd
}

fn run_gn_gen(gn_args: &[String]) -> PathBuf {
  let dirs = get_dirs();
  let gn_out_dir = dirs.out.join("gn_out");

  let mut args = gn_args.join(" ");
  if let Ok(extra_args) = env::var("EXTRA_GN_ARGS") {
    args.push(' ');
    args.push_str(&extra_args);
  }

  let path = env::current_dir().unwrap();
  println!("The current directory is {}", path.display());
  println!(
    "gn gen --root={} {}",
    dirs.root.display(),
    gn_out_dir.display()
  );
  assert!(
    Command::new(gn())
      .arg(format!("--root={}", dirs.root.display()))
      .arg(format!("--script-executable={}", python()))
      .arg("gen")
      .arg(&gn_out_dir)
      .arg("--ide=json")
      .arg("--args=".to_owned() + &args)
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .envs(env::vars())
      .status()
      .expect("Could not run `gn`")
      .success()
  );

  gn_out_dir
}

pub fn build(target: &str, maybe_env: Option<NinjaEnv>) {
  let gn_out_dir = get_dirs().out.join("gn_out");

  rerun_if_changed(&gn_out_dir, maybe_env.clone(), target);

  // This helps Rust source files locate the snapshot, source map etc.
  println!("cargo:rustc-env=GN_OUT_DIR={}", gn_out_dir.display());

  assert!(
    ninja(&gn_out_dir, maybe_env)
      .arg(target)
      .status()
      .unwrap()
      .success()
  );

  // TODO This is not sufficient. We need to use "gn desc" to query the target
  // and figure out what else we need to add to the link.
  println!(
    "cargo:rustc-link-search=native={}/obj/",
    gn_out_dir.display()
  );
}

/// build.rs does not get re-run unless we tell cargo about what files we
/// depend on. This outputs a bunch of rerun-if-changed lines to stdout.
fn rerun_if_changed(out_dir: &Path, maybe_env: Option<NinjaEnv>, target: &str) {
  let deps = ninja_get_deps(out_dir, maybe_env, target);
  for d in deps {
    if let Ok(p) = out_dir.join(d).canonicalize() {
      println!("cargo:rerun-if-changed={}", p.display());
    }
  }
}

fn ninja_get_deps(
  out_dir: &Path,
  maybe_env: Option<NinjaEnv>,
  target: &str,
) -> HashSet<String> {
  let mut cmd = ninja(out_dir, maybe_env.clone());
  cmd.arg("-t");
  cmd.arg("graph");
  cmd.arg(target);
  let output = cmd.output().expect("ninja -t graph failed");
  let stdout = String::from_utf8(output.stdout).unwrap();
  let graph_files = parse_ninja_graph(&stdout);

  let mut cmd = ninja(out_dir, maybe_env);
  cmd.arg(target);
  cmd.arg("-t");
  cmd.arg("deps");
  let output = cmd.output().expect("ninja -t deps failed");
  let stdout = String::from_utf8(output.stdout).unwrap();
  let deps_files = parse_ninja_deps(&stdout);

  graph_files.union(&deps_files).map(String::from).collect()
}

pub fn parse_ninja_deps(s: &str) -> HashSet<String> {
  let mut out = HashSet::new();
  for line in s.lines() {
    if line.starts_with("  ") {
      let filename = line.trim().to_string();
      out.insert(filename);
    }
  }
  out
}

/// A parser for the output of "ninja -t graph". It returns all the input files.
pub fn parse_ninja_graph(s: &str) -> HashSet<String> {
  let mut out = HashSet::new();
  // This is extremely hacky and likely to break.
  for line in s.lines() {
    if line.starts_with('\"')
      && line.contains("label=")
      && !line.contains("shape=")
      && !line.contains(" -> ")
    {
      let filename = line.split('\"').nth(3).unwrap();
      if !filename.starts_with("..") {
        continue;
      }
      out.insert(filename.to_string());
    }
  }
  out
}

fn env_bool(key: &str) -> bool {
  matches!(
    env::var(key).unwrap_or_default().as_str(),
    "true" | "1" | "yes"
  )
}

#[cfg(test)]
mod test {
  use super::*;

  fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
      .downcast_ref::<String>()
      .map(String::as_str)
      .or_else(|| payload.downcast_ref::<&str>().copied())
      .unwrap()
  }

  const MOCK_GRAPH: &str = r#"
digraph ninja {
rankdir="LR"
node [fontsize=10, shape=box, height=0.25]
edge [fontsize=10]
"0x7fc3c040c210" [label="default"]
"0x7fc3c040a7f0" -> "0x7fc3c040c210" [label=" phony"]
"0x7fc3c040a7f0" [label="obj/default.stamp"]
"0x7fc3c040a790" [label="stamp", shape=ellipse]
"0x7fc3c040a790" -> "0x7fc3c040a7f0"
"0x7fc3c040a6c0" -> "0x7fc3c040a790" [arrowhead=none]
"0x7fc3c040a8a0" -> "0x7fc3c040a790" [arrowhead=none]
"0x7fc3c040a920" -> "0x7fc3c040a790" [arrowhead=none]
"0x7fc3c040a6c0" [label="obj/count_bytes.stamp"]
"0x7fc3c040a4d0" -> "0x7fc3c040a6c0" [label=" stamp"]
"0x7fc3c040a4d0" [label="gen/output.txt"]
"0x7fc3c040a400" [label="___count_bytes___build_toolchain_mac_clang_x64__rule", shape=ellipse]
"0x7fc3c040a400" -> "0x7fc3c040a4d0"
"0x7fc3c040a580" -> "0x7fc3c040a400" [arrowhead=none]
"0x7fc3c040a620" -> "0x7fc3c040a400" [arrowhead=none]
"0x7fc3c040a580" [label="../../../example/src/count_bytes.py"]
"0x7fc3c040a620" [label="../../../example/src/input.txt"]
"0x7fc3c040a8a0" [label="foo"]
"0x7fc3c040b5e0" [label="link", shape=ellipse]
"0x7fc3c040b5e0" -> "0x7fc3c040a8a0"
"0x7fc3c040b5e0" -> "0x7fc3c040b6d0"
"0x7fc3c040b5e0" -> "0x7fc3c040b780"
"0x7fc3c040b5e0" -> "0x7fc3c040b820"
"0x7fc3c040b020" -> "0x7fc3c040b5e0" [arrowhead=none]
"0x7fc3c040a920" -> "0x7fc3c040b5e0" [arrowhead=none]
"0x7fc3c040b020" [label="obj/foo/foo.o"]
"0x7fc3c040b0d0" -> "0x7fc3c040b020" [label=" cxx"]
"0x7fc3c040b0d0" [label="../../../example/src/foo.cc"]
"0x7fc3c040a920" [label="obj/libhello.a"]
"0x7fc3c040be00" -> "0x7fc3c040a920" [label=" alink"]
"0x7fc3c040be00" [label="obj/hello/hello.o"]
"0x7fc3c040beb0" -> "0x7fc3c040be00" [label=" cxx"]
"0x7fc3c040beb0" [label="../../../example/src/hello.cc"]
}
  "#;

  #[test]
  fn test_parse_ninja_graph() {
    let files = parse_ninja_graph(MOCK_GRAPH);
    assert!(files.contains("../../../example/src/input.txt"));
    assert!(files.contains("../../../example/src/count_bytes.py"));
    assert!(!files.contains("obj/hello/hello.o"));
  }

  #[test]
  fn test_resolve_nimbus_prebuilt_version() {
    assert_eq!(
      resolve_prebuilt_version("150.4.0", None),
      "150.4.0-nimbus.1"
    );
    assert_eq!(
      resolve_prebuilt_version("150.4.0", Some("150.4.0-nimbus.7".to_string())),
      "150.4.0-nimbus.7"
    );
    assert_eq!(
      prebuilt_asset_url(
        DEFAULT_NIMBUS_PREBUILT_BASE,
        "150.4.0-nimbus.1",
        "src_binding_release_aarch64-apple-darwin.rs",
      ),
      "https://github.com/nimbus/rusty_v8/releases/download/\
       v150.4.0-nimbus.1/src_binding_release_aarch64-apple-darwin.rs"
    );
    assert_eq!(
      downloaded_binding_path(
        Path::new("target/build/out"),
        "src_binding_release_aarch64-apple-darwin.rs"
      ),
      Path::new("target/build/out/src_binding_release_aarch64-apple-darwin.rs")
    );
    assert_eq!(
      packaged_binding_path(
        Path::new("crate"),
        "src_binding_release_aarch64-apple-darwin.rs"
      ),
      Path::new("crate/gen/src_binding_release_aarch64-apple-darwin.rs")
    );
    assert_eq!(
      src_binding_name("aarch64-apple-ios", "debug", "_sandbox"),
      "src_binding_sandbox_debug_aarch64-apple-ios.rs"
    );
  }

  #[test]
  fn test_nimbus_prebuilt_selectors() {
    assert_eq!(prebuilt_features_suffix_from_flags(false, false, false), "");
    assert_eq!(
      prebuilt_features_suffix_from_flags(true, false, true),
      "_ptrcomp_simdutf"
    );
    assert_eq!(
      prebuilt_features_suffix_from_flags(true, false, false),
      "_ptrcomp"
    );
    assert_eq!(
      prebuilt_features_suffix_from_flags(true, true, true),
      "_ptrcomp_sandbox_simdutf"
    );

    for target in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"] {
      assert_eq!(
        prebuilt_archive_name(target, "release", "").unwrap(),
        format!("librusty_v8_release_{target}.a.gz")
      );
      assert_eq!(
        prebuilt_archive_name(target, "release", "_simdutf").unwrap(),
        format!("librusty_v8_simdutf_release_{target}.a.gz")
      );
      assert_eq!(
        prebuilt_binding_name(target, "release", "_simdutf").unwrap(),
        format!("src_binding_simdutf_release_{target}.rs")
      );
      assert!(
        prebuilt_archive_name(target, "release", "_ptrcomp_simdutf")
          .unwrap_err()
          .contains("does not publish")
      );
      assert!(
        prebuilt_binding_name(target, "release", "_sandbox")
          .unwrap_err()
          .contains("does not publish")
      );
    }

    for (target, features) in [
      ("x86_64-unknown-linux-gnu", "_sandbox"),
      ("aarch64-unknown-linux-gnu", "_ptrcomp_sandbox_simdutf"),
      ("x86_64-pc-windows-msvc", "_ptrcomp_simdutf"),
      ("mips64-unknown-linux-gnu", ""),
    ] {
      let error =
        prebuilt_archive_name(target, "release", features).unwrap_err();
      assert!(error.contains("does not publish"));
      assert!(error.contains("V8_FROM_SOURCE=1"));
    }

    assert!(
      prebuilt_archive_name("aarch64-apple-darwin", "release", "_ptrcomp")
        .is_ok()
    );
    assert!(
      prebuilt_archive_name(
        "aarch64-apple-darwin",
        "release",
        &prebuilt_features_suffix_from_flags(true, false, true)
      )
      .is_ok()
    );

    for name in [
      prebuilt_archive_name("aarch64-apple-darwin", "debug", ""),
      prebuilt_binding_name("aarch64-apple-darwin", "debug", ""),
    ] {
      let error = name.unwrap_err();
      assert!(error.contains("profile debug"));
      assert!(error.contains("V8_FROM_SOURCE=1"));
    }
  }

  #[test]
  fn test_custom_archive_requires_matching_binding() {
    assert!(validate_custom_archive_overrides(false, false).is_ok());
    assert!(validate_custom_archive_overrides(false, true).is_ok());
    assert!(validate_custom_archive_overrides(true, true).is_ok());
    assert!(
      validate_custom_archive_overrides(true, false)
        .unwrap_err()
        .contains("RUSTY_V8_SRC_BINDING_PATH")
    );
  }

  #[test]
  fn test_nimbus_prebuilt_checksum_contract() {
    const DIGEST: &str =
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    assert_eq!(
      parse_sha256_sidecar(&format!("{DIGEST}  asset.bin\n"), "asset.bin")
        .unwrap(),
      DIGEST
    );
    assert_eq!(
      asset_name_from_url(r"C:\Users\runner\cache\asset.bin"),
      "asset.bin"
    );
    assert!(
      parse_sha256_sidecar(&format!("{DIGEST}  different.bin\n"), "asset.bin")
        .unwrap_err()
        .contains("expected asset.bin")
    );
    assert!(
      parse_sha256_sidecar("not-a-digest  asset.bin\n", "asset.bin")
        .unwrap_err()
        .contains("64 hexadecimal digits")
    );

    let missing_sidecar_path = std::env::temp_dir().join(format!(
      "rusty-v8-missing-sidecar-{}-nimbus-v8-asset.a.sha256",
      std::process::id()
    ));
    assert!(!missing_sidecar_path.exists());
    let missing_sidecar = std::panic::catch_unwind(|| {
      download_checksum_sidecar(
        missing_sidecar_path.to_str().unwrap(),
        Path::new("unused"),
      );
    })
    .unwrap_err();
    let missing_sidecar = panic_message(missing_sidecar.as_ref());
    assert!(missing_sidecar.contains("missing the required checksum sidecar"));
    assert!(missing_sidecar.contains("nimbus-v8-asset.a.sha256"));

    let root = std::env::temp_dir()
      .join(format!("rusty-v8-checksum-contract-{}", std::process::id()));
    if root.exists() {
      fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    let asset = root.join("asset.bin");
    let output = root.join("output.bin");
    fs::write(&asset, b"abc").unwrap();
    fs::write(
      root.join("asset.bin.sha256"),
      format!("{DIGEST}  asset.bin\n"),
    )
    .unwrap();

    download_file(asset.to_str().unwrap(), &output, ChecksumPolicy::Required);
    assert_eq!(fs::read(&output).unwrap(), b"abc");

    let mismatch_asset = root.join("mismatch.bin");
    let mismatch_output = root.join("mismatch-output.bin");
    fs::write(&mismatch_asset, b"substituted").unwrap();
    fs::write(
      root.join("mismatch.bin.sha256"),
      format!("{DIGEST}  mismatch.bin\n"),
    )
    .unwrap();
    let mismatch = std::panic::catch_unwind(|| {
      download_file(
        mismatch_asset.to_str().unwrap(),
        &mismatch_output,
        ChecksumPolicy::Required,
      );
    })
    .unwrap_err();
    assert!(
      panic_message(mismatch.as_ref())
        .contains("SHA-256 mismatch for V8 prebuilt asset")
    );

    // A modified output must not be accepted as a cache hit.
    fs::write(&output, b"tampered").unwrap();
    download_file(asset.to_str().unwrap(), &output, ChecksumPolicy::Required);
    assert_eq!(fs::read(&output).unwrap(), b"abc");

    let trusted_asset = root.join("trusted.a");
    let trusted_output = root.join("trusted-output.a");
    fs::write(&trusted_asset, b"caller-trusted").unwrap();
    download_file(
      trusted_asset.to_str().unwrap(),
      &trusted_output,
      ChecksumPolicy::TrustedCustomArchive,
    );
    assert_eq!(fs::read(&trusted_output).unwrap(), b"caller-trusted");
    fs::write(&trusted_asset, b"caller-trusted-updated").unwrap();
    download_file(
      trusted_asset.to_str().unwrap(),
      &trusted_output,
      ChecksumPolicy::TrustedCustomArchive,
    );
    assert_eq!(
      fs::read(&trusted_output).unwrap(),
      b"caller-trusted-updated"
    );

    let remote_url = "https://example.invalid/caller-trusted.a";
    fs::write(&trusted_output, b"cached-remote").unwrap();
    let cached_remote_digest = sha256_file(&trusted_output).unwrap();
    fs::write(
      static_checksum_path(&trusted_output),
      format!("{remote_url}\ntrusted-custom-archive\n{cached_remote_digest}\n"),
    )
    .unwrap();
    assert!(cached_download_is_reusable(
      remote_url,
      &trusted_output,
      ChecksumPolicy::TrustedCustomArchive,
    ));
    assert!(!cached_download_is_reusable(
      trusted_asset.to_str().unwrap(),
      &trusted_output,
      ChecksumPolicy::TrustedCustomArchive,
    ));

    fs::remove_dir_all(root).unwrap();
  }
}
