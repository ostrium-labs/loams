# The local nightly timer

The nightly runs on the laptops, not in CI (owner, 2026-10-28). These two
units run [`scripts/nightly/run-local.sh`](../../scripts/nightly/run-local.sh)
every night as a systemd **user** service. Nothing installs them; to do so:

```sh
# 1. A worktree that tracks dev, used only by the nightly.
git -C ~/Documents/Ostriumlabs/loams worktree add ~/Documents/Ostriumlabs/loams-wt/nightly origin/dev --detach

# 2. The units, linked into the user's systemd directory.
mkdir -p ~/.config/systemd/user
ln -sf ~/Documents/Ostriumlabs/loams-wt/nightly/deploy/nightly/loams-nightly.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now loams-nightly.timer

# 3. Keep the user's services running while logged out (once per machine).
loginctl enable-linger "$USER"
```

Then:

```sh
systemctl --user list-timers loams-nightly.timer   # the next run
systemctl --user start loams-nightly.service       # run now
journalctl --user -u loams-nightly -f              # follow a run
systemctl --user disable --now loams-nightly.timer # stop the nightly
```

The results go to `target/nightly/<date>-<commit>/` in the nightly worktree.
The service changes `docs/guides/compute/compatibility.md` in that worktree
only; commit the page by hand when a run should be published (owner ruling,
2026-10-27). `LOAMS_NIGHTLY_CHECKOUT` in the service points at the worktree.
The service runs at idle I/O priority, and every heavy step waits for the
shared build-slot limiter.
