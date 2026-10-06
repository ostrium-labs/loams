# Loams Desktop identity assets

The existing Loams placeholder L-and-dot mark is the canonical desktop icon. `dev.loams.desktop.svg` is used to generate the shared PNG and Windows icon; the monochrome UI logo and pixel loader use the same identity.

`loams-desktop.desktop` carries the application ID `dev.loams.desktop`, executable `loams-desktop`, and `loams://` handler. The packaged launcher at `dist/loams-desktop.desktop` uses the installed icon filename and `%u` for single-URL CLI routing. Existing Loams identity values live in `crates/loams-desktop-brand`.
