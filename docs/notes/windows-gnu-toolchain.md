# Building x86_64-pc-windows-gnu on Windows without installing MinGW

**Lesson: use the GNU HOST toolchain (`1.97.1-x86_64-pc-windows-gnu`) for ship
builds; adding the GNU TARGET to the MSVC toolchain cannot link. Keep the repo-wide
toolchain pin host-neutral so macOS development still works.**

- The msvc host toolchain + gnu target std ships only `crt2.o`/`dllcrt2.o` in
  `self-contained/` — no system-DLL import libs (`libkernel32.a` …), so linking fails
  with `unable to find library -lkernel32` even with `rust-lld` +
  `link-self-contained=yes`.
- The gnu HOST toolchain bundles `rust-mingw` (binutils ld + full import-lib set) and is
  fully self-sufficient for pure-Rust crates. Install/select
  `1.97.1-x86_64-pc-windows-gnu` for the ship command; its default build target
  is already the ship target.
- Baseline imports of a std hello-world gnu exe (measured 2026-07-04): `KERNEL32.dll`,
  `msvcrt.dll`, `ntdll.dll`, `api-ms-win-core-synch-l1-2-0.dll`. All ship with Windows;
  the gate allowlist includes them (SPEC §12.3). `libgcc`/`winpthread` are statically
  linked — no extra DLLs.
- Baseline size with the ship release profile (opt-level=z, lto, cgu=1, panic=abort,
  strip): ~234 KB for hello-world — leaves ~1.2 MB of the 1,474,560-byte budget.
- rustup `--profile minimal` omits clippy/rustfmt — add them explicitly:
  `rustup component add clippy rustfmt --toolchain 1.97.1-x86_64-pc-windows-gnu`.
