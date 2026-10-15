//! Builds espeak-ng statically from the exact upstream commit that
//! piper-tts 1.4.2 uses (OHF-Voice/piper1-gpl v1.4.2 CMakeLists.txt), so Piper
//! phonemes match the voices' training/reference pipeline. The Debian 1.52.0
//! release differs (e.g. `ɾ` instead of `r` after plosives).
//!
//! Source: `$OPENREADER_ESPEAK_SRC` (an unpacked espeak-ng tree, for offline
//! builds) or a download of the pinned commit from GitHub (cached in OUT_DIR).
//! Needs CMake and a C compiler. The compiled `espeak-ng-data` (core files,
//! `lang/`, `voices/` and the dictionaries listed in ESPEAK_DICTS) is embedded
//! into the binary and unpacked on first use.

use std::io::Write;
use std::path::{Path, PathBuf};

const ESPEAK_COMMIT: &str = "212928b394a96e8fd2096616bfd54e17845c48f6";
/// Dictionaries embedded by default; override with `OPENREADER_ESPEAK_DICTS=de,en,fr`.
const ESPEAK_DICTS: &str = "de,en";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OPENREADER_ESPEAK_SRC");
    println!("cargo:rerun-if-env-changed=OPENREADER_ESPEAK_DICTS");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let source = match std::env::var_os("OPENREADER_ESPEAK_SRC") {
        Some(path) => copy_source(Path::new(&path), &out.join("espeak-ng-local")),
        None => download_source(&out),
    };

    // espeak-ng's data rules use `${CMAKE_BINARY_DIR}/../phsource`, so the
    // build directory must be `<source>/build`: build (and install) in the tree.
    // Its data compiler also uses fixed-size path buffers, so the tree must live
    // under a short path (deep `target/` paths overflow them).
    let short = std::env::var_os("OPENREADER_ESPEAK_BUILD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            out.hash(&mut hasher);
            std::env::temp_dir().join(format!("oresp-{:x}", hasher.finish()))
        });
    let _ = std::fs::remove_dir_all(&short);
    let source = copy_source(&source, &short);
    let install = cmake::Config::new(&source)
        .out_dir(&source)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
        .define("USE_ASYNC", "OFF")
        .define("USE_MBROLA", "OFF")
        .define("USE_LIBSONIC", "OFF")
        .define("USE_LIBPCAUDIO", "OFF")
        .define("USE_KLATT", "OFF")
        .define("USE_SPEECHPLAYER", "OFF")
        .define("ESPEAK_COMPAT", "OFF")
        .define("BUILD_TESTING", "OFF")
        .cflag("-D_FILE_OFFSET_BITS=64")
        .profile("Release")
        .build();

    // Move the results into OUT_DIR and drop the temporary tree.
    let result = out.join("espeak");
    let _ = std::fs::remove_dir_all(&result);
    std::fs::create_dir_all(&result).unwrap();
    for library in ["libespeak-ng.a", "libucd.a"] {
        let found = find_file(&install, library).unwrap_or_else(|| panic!("{library} not built"));
        std::fs::copy(found, result.join(library)).unwrap();
    }
    let data = install.join("share/espeak-ng-data");
    assert!(data.join("phontab").exists(), "espeak-ng data was not built: {}", data.display());
    let status = std::process::Command::new("cp")
        .arg("-a")
        .arg(&data)
        .arg(result.join("espeak-ng-data"))
        .status()
        .unwrap();
    assert!(status.success());
    let _ = std::fs::remove_dir_all(&short);

    println!("cargo:rustc-link-search=native={}", result.display());
    println!("cargo:rustc-link-lib=static=espeak-ng");
    println!("cargo:rustc-link-lib=static=ucd");
    embed_data(&result.join("espeak-ng-data"), &out.join("espeak_data.rs"));
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
    }
    None
}

fn copy_source(from: &Path, to: &Path) -> PathBuf {
    if !to.join("CMakeLists.txt").exists() {
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg(from)
            .arg(to)
            .status()
            .expect("cp espeak-ng source");
        assert!(status.success(), "copying {} failed", from.display());
    }
    to.to_path_buf()
}

fn download_source(out: &Path) -> PathBuf {
    let target = out.join(format!("espeak-ng-{ESPEAK_COMMIT}"));
    if target.join("CMakeLists.txt").exists() {
        return target;
    }
    let url = format!("https://codeload.github.com/espeak-ng/espeak-ng/tar.gz/{ESPEAK_COMMIT}");
    let response = ureq::get(&url)
        .call()
        .unwrap_or_else(|error| panic!("espeak-ng download failed ({url}): {error}. Set OPENREADER_ESPEAK_SRC for offline builds."));
    let decoder = flate2::read::GzDecoder::new(response.into_reader());
    tar::Archive::new(decoder)
        .unpack(out)
        .expect("unpack espeak-ng source");
    assert!(target.join("CMakeLists.txt").exists(), "unexpected archive layout");
    target
}

fn embed_data(data: &Path, output: &Path) {
    let dicts: Vec<String> = std::env::var("OPENREADER_ESPEAK_DICTS")
        .unwrap_or_else(|_| ESPEAK_DICTS.into())
        .split(',')
        .map(|dict| format!("{}_dict", dict.trim()))
        .collect();
    let mut files = Vec::new();
    collect(data, data, &dicts, &mut files);
    files.sort();
    let mut code = std::fs::File::create(output).unwrap();
    writeln!(code, "/// Embedded espeak-ng-data (relative path, contents).").unwrap();
    writeln!(code, "pub static ESPEAK_DATA: &[(&str, &[u8])] = &[").unwrap();
    for (relative, absolute) in files {
        writeln!(code, "    ({relative:?}, include_bytes!({:?})),", absolute.display().to_string()).unwrap();
    }
    writeln!(code, "];").unwrap();
    writeln!(code, "pub const ESPEAK_COMMIT: &str = {ESPEAK_COMMIT:?};").unwrap();
}

fn collect(root: &Path, dir: &Path, dicts: &[String], files: &mut Vec<(String, PathBuf)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            collect(root, &path, dicts, files);
        } else if !relative.ends_with("_dict") || dicts.contains(&relative) {
            files.push((relative, path));
        }
    }
}
