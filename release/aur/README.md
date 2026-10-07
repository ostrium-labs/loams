![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# AUR packaging for Loams

[`PKGBUILD`](PKGBUILD) is the AUR recipe. It lives **here, in the Loams
repository**, not in the AUR — the AUR is a public registry and publishing under
`ostrium-labs` is a human decision. This file is what has to be true first, and
how to get from here to a submitted package.

The rest of the packaging is [`docs/release/packaging.md`](../../docs/release/packaging.md).
What is signed is [`docs/release/signing.md`](../../docs/release/signing.md).

## Two different things, and which one you want

| | The published pacman repository | The AUR |
|---|---|---|
| What the user installs | `sudo pacman -U loams-0.0.1-1-x86_64.pkg.tar.zst` | `yay -S loams` |
| Where the binary comes from | Loams CI, already built | Built on the user's machine |
| Signature | `loams-….pkg.tar.zst.sig`, signed with the project GPG key | None — an AUR build is built and installed locally, and makepkg does not sign what it builds |
| Needs network to install | No, after download | No, but it downloads crates and compiles ~500 of them |
| Exists today | Packages are built by `release-package.yml`; the repository is **not published** — see [`docs/release/packaging.md`](../../docs/release/packaging.md#repositories) | **Not submitted.** This is the work this file describes. |

**Prefer the published repository when it exists.** It is a prebuilt binary with a
signature; the AUR path compiles DataFusion, Lance and Tantivy from scratch and
takes a long time on a cold cache.

## What has to be true before submitting

1. **A published, signed version.** The AUR package installs `loams` from a
   source tarball of a tag. `pkgver` must be a tag that exists, and the tag's
   version must be the workspace version — the same rule `release-package.yml`'s
   guard enforces.
2. **`pkgver` and `pkgrel` updated.** `PKGBUILD` is pinned at `0.0.1`. Bump
   `pkgver` per release and `pkgrel` to 1, then increment `pkgrel` for a
   packaging-only change with no version bump.
3. **A maintainer account that can submit.** The AUR RPC interface rejects any
   submitter who is not an **AUR Trusted User**, or the package's own
   maintainer, once it is adopted. There is no such account for `ostrium-labs`
   today.
4. **The three roles `ostrium-labs` already has to define** for SignPath's
   Foundation application (Authors, Reviewers, Approvers) — named on the project
   home page. See [`signing.md`](../../docs/release/signing.md#the-three-roles-the-foundation-requires).
   An AUR package is reviewed by people outside the project, which is exactly
   what that policy is for.

**None of these can be automated from this repository with the automation token
available.** They are listed so the work is visible rather than implied.

## How to submit, once the above is true

The AUR is a bare git repository. Submitting means pushing a commit that adds
one file:

```sh
# 1. Clone the (empty) package repository.
git clone https://aur.archlinux.org/loams.git
cd loams

# 2. Copy the recipe in, with the version already bumped.
cp /path/to/loams/release/aur/PKGBUILD ./PKGBUILD

# 3. Commit. The AUR requires the commit message to be exactly the package
#    name and version, in the form "[pkg] 1.0.0-1".
git add PKGBUILD
git commit -m '[pkg] 0.0.1-1'

# 4. Push.
git push
```

If the package name is already taken, the push is rejected and the recipe has to
change `pkgname`; `ostrium-labs` should decide that deliberately rather than by
picking the first free name.

## Keeping it in step with the repo

`PKGBUILD` is a **source copy** of a file the packages are built from. That is
the maintenance cost of the AUR, and it should be stated rather than hidden:

- The `depends` list, the feature set and the systemd units must match
  `release/nfpm.yaml`. If `depends` there changes, this file changes too.
- `pkgdesc`, `license` and `url` are the same values `release/nfpm.yaml`
  declares, so a rename in one is a rename in both.

`release/nfpm.yaml` and `PKGBUILD` do not currently have a test that compares
them. **That is a known gap**, not a decision: a maintainer who changes one
without the other gets a package whose dependency list is quietly wrong.

## Testing a PKGBUILD before submitting

`makepkg` runs the recipe on the build machine, and `namcap` checks it against
the AUR's guidelines. Both run on Arch and CachyOS:

```sh
sudo pacman -S --needed base-devel git namcap

git clone https://github.com/ostrium-labs/loams.git
cd loams/release/aur
cp PKGBUILD /tmp/build/ && cd /tmp/build
makepkg --printsrcinfo > .SRCINFO   # what gets uploaded to the AUR
makepkg -si                         # build, install, ask
```

`makepkg` needs `Cargo.lock` and `rust-toolchain.toml`, both of which are in the
tarball, so it pins Rust 1.97.1 through rustup on first build. `protoc` comes
from `makedepends`; the six `build.rs` scripts that need it are listed in
[`docs/build-from-source/windows.md`](../../docs/build-from-source/windows.md#4-protoc).

Run `namcap PKGBUILD` and fix what it reports before submitting. Two warnings to
expect, both of which are deliberate and should not be "fixed":

- `depends-on-udev` will not appear, and `provides=('loams-server')` will draw a
  `checkdepends`-style comment about not being a real provider name.
- `noextract` may appear for the archive, and is fine for a source tarball.
