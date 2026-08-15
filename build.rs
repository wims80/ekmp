use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=assets/windows/ekmp.rc");
    println!("cargo:rerun-if-changed=assets/windows/app-icon.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let resource = output.join("ekmp.res");
    let status = compile_resource(Path::new("rc.exe"), &resource)
        .or_else(|error| {
            if error.kind() == ErrorKind::NotFound {
                let compiler = windows_sdk_resource_compiler().ok_or(error)?;
                compile_resource(&compiler, &resource)
            } else {
                Err(error)
            }
        })
        .expect("could not run rc.exe; install the Windows SDK");

    assert!(
        status.success(),
        "rc.exe could not compile the application icon"
    );
    println!("cargo:rustc-link-arg={}", resource.display());
}

fn compile_resource(compiler: &Path, resource: &Path) -> std::io::Result<std::process::ExitStatus> {
    Command::new(compiler)
        .current_dir("assets/windows")
        .arg("/nologo")
        .arg("/fo")
        .arg(resource)
        .arg("ekmp.rc")
        .status()
}

fn windows_sdk_resource_compiler() -> Option<PathBuf> {
    let kits_bin = PathBuf::from(env::var_os("ProgramFiles(x86)")?)
        .join("Windows Kits")
        .join("10")
        .join("bin");
    let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => "x64",
    };

    let mut versions = fs::read_dir(&kits_bin)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    versions.sort();
    versions.reverse();

    versions
        .into_iter()
        .map(|version| version.join(architecture).join("rc.exe"))
        .find(|candidate| candidate.is_file())
        .or_else(|| {
            let candidate = kits_bin.join(architecture).join("rc.exe");
            candidate.is_file().then_some(candidate)
        })
}
