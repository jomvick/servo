# FreeType binding compatibility

Komet's pinned GPUI revision depends on `zed-font-kit` at
`94b0f28166665e8fd2f53ff6d268a14955c82269`, which requests `freetype-sys` 0.20.
Servo uses 0.23. Both native crates declare `links = "freetype"`, so Cargo cannot
resolve them together even when both use the same system FreeType.

This facade has the older package version but reexports the newer binding. It
has no build script and no `links` declaration. Every FreeType symbol and FFI
type therefore comes from the single 0.23 binding, used directly by Servo and
indirectly by GPUI. Keep the root `[patch.crates-io]` entry when embedding in a
host workspace. Remove this facade when GPUI's pinned font loader accepts the
same `freetype-sys` release as Servo.
