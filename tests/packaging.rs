//! Checks that packaging metadata tracks the Cargo package version.

#[test]
fn pkgbuild_version_matches_cargo() {
    let pkgbuild = include_str!("../packaging/arch/PKGBUILD");
    let pkgver = pkgbuild
        .lines()
        .find_map(|line| line.strip_prefix("pkgver="))
        .expect("PKGBUILD sets pkgver");
    assert_eq!(
        pkgver,
        env!("CARGO_PKG_VERSION"),
        "bump pkgver in packaging/arch/PKGBUILD together with Cargo.toml"
    );
}
