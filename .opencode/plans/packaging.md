# term-wm — Distribution & Packaging Plan

**Current status:** Published on [crates.io](https://crates.io/crates/term-wm) (v0.8.20-alpha).  
`cargo install term-wm` works today.

---

## Target Package Managers

| Manager | Effort | How |
|---|---|---|
| **Homebrew** | Low–Medium | Write a Ruby formula in a personal tap (`homebrew-term-wm`) or submit to `homebrew-core`. Formula: `desc`, `license`, `depends_on "rust" => :build`, `system "cargo", "install", *std_cargo_args`. |
| **AUR (Arch Linux)** | Low | Write a PKGBUILD — trivial for Rust projects. `build()` runs `cargo build --release`, `package()` runs `cargo install`. Submit via `git push` to aur@archlinux.org. |
| **Nixpkgs** | Medium | Write a Nix derivation using `rustPlatform.buildRustPackage`. Submit PR to [Nixpkgs](https://github.com/NixOS/nixpkgs). |
| **Debian/Ubuntu (apt)** | High | Debian packaging (`debian/control`, `debian/rules`, `debian/copyright`, `debian/changelog`). Needs Debian Developer sponsor. Upload via mentors.debian.net. |
| **Fedora (dnf)** | High | RPM spec file. Package review through [Fedora Package Review](https://fedoraproject.org/wiki/Package_Review_Process). Needs Fedora packager sponsor. |
| **Scoop (Windows)** | Low | JSON manifest in the [Scoop bucket](https://github.com/ScoopInstaller/Main) — `cargo install` based. |
| **Snapcraft** | Medium | `snapcraft.yaml` with `cargo` plugin. Publish to Snap Store. |
| **Docker / GHCR** | Low | Multi-stage Dockerfile: `rust:alpine` → build, distroless runtime image. |

---

## Recommended Path (least friction first)

1. **Homebrew tap** — own it, zero approval process
2. **AUR PKGBUILD** — one file, Arch users get it immediately
3. **Scoop manifest** — easy Windows coverage
4. **Homebrew core PR** — submit upstream once the tap is stable
5. **Nixpkgs PR** — medium effort, good for Nix audience

---

## Prerequisites

- `cargo build --release` builds cleanly on a fresh system with only Rust toolchain
- Binary name is `term-wm` (stable CLI interface, well-documented `--help`)
- MIT/Apache-2.0 dual license is fine for all major distros
