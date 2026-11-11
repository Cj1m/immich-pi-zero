# immich-pi-zero

An unofficial server. This project is not Immich, and it is not affiliated with the Immich project or FUTO. Immich is a trademark of FUTO.

A Rust server for a Raspberry Pi Zero 2 W. The official Immich mobile app and the official Immich v3.3.6 web UI sign in, back up photos and videos, and browse them here.

This is the server only. The web UI is the v3.3.6 build from [Immich](https://github.com/immich-app/immich). Face detection, OCR, and smart search run in the official v3.3.6 machine-learning container, on another computer. The Pi stores the results.

The database is a SQLite file in the data directory. Thumbnails stay on the Pi. Originals stay on the Pi, or, when object storage is configured, are sealed with AES-256-GCM and uploaded one at a time. Playback sends the original file. `ffmpeg` is used to grab one poster frame from a video.

Slopped together with Grok 4.7.

## Requirements

- Rust 1.89 or newer, on a machine with enough memory to compile. The Pi Zero 2 W is the machine that runs the binary.
- `ffmpeg` on the Pi's `PATH`, for video posters.
- Node 22 and pnpm 11, only while building the web UI.
- Docker, on the machine that runs machine learning. Leave that container off the Pi.

A Pi build also needs Zig 0.17 on the build machine. The binary is an aarch64 ELF linked for glibc 2.36 or older, which Debian 13 provides.

## Configure

The process reads environment variables. A file named `immich.env` next to the binary is a convenient place to keep them. The first start creates the admin only when the database has no user, and only when the three admin variables are set.

```sh
IMMICH_DATA_DIR=/home/USER/immich/data
IMMICH_WEB_ROOT=/home/USER/immich/web
IMMICH_HOST=0.0.0.0
IMMICH_PORT=2283

IMMICH_ADMIN_EMAIL=you@example.com
IMMICH_ADMIN_NAME=Admin
IMMICH_ADMIN_PASSWORD=change-me

# Leave these unset to keep originals as ordinary files in $IMMICH_DATA_DIR/objects.
IMMICH_S3_ENDPOINT=https://s3.example.com
IMMICH_S3_BUCKET=library
IMMICH_S3_REGION=us-east-1
IMMICH_S3_ACCESS_KEY_ID=
IMMICH_S3_SECRET_ACCESS_KEY=
# Set to true for a path-style endpoint. Backblaze B2 stays unset (virtual-hosted).
IMMICH_S3_FORCE_PATH_STYLE=false

# The machine running the v3.3.6 machine-learning container. Port 3003.
IMMICH_MACHINE_LEARNING_URL=http://192.168.1.10:3003
# The only value that turns machine learning off is the string false.
IMMICH_MACHINE_LEARNING_ENABLED=true
```

Replace `USER` and the addresses. `IMMICH_HOST` defaults to `0.0.0.0`, `IMMICH_PORT` to `2283`, `IMMICH_DATA_DIR` to `data`, and `IMMICH_WEB_ROOT` to `web`.

The admin variables are read only when the database has no user. Changing them later does not reset the password.

With the four `IMMICH_S3_*` values set, the first start creates `data/keys/aes-256.key`. Copy that file somewhere safe. It is the only key that can open sealed originals. A later install must keep the same file. Thumbnails are never sealed.

## Web UI

Clone Immich at the v3.3.6 tag and build the web package. From that clone:

```sh
pnpm install --frozen-lockfile --filter immich-web...
pnpm --filter @immich/sdk build
pnpm --filter immich-web build
```

Copy `web/build/` to the directory named by `IMMICH_WEB_ROOT`. The server serves those files as-is.

## Machine learning

On a computer other than the Pi:

```sh
docker run -d \
  --name immich-machine-learning \
  --restart unless-stopped \
  -p 3003:3003 \
  -v "$HOME/immich-ml/cache:/cache" \
  -e MACHINE_LEARNING_MODEL_TTL=0 \
  -e MACHINE_LEARNING_PRELOAD__CLIP__VISUAL=ViT-B-32__openai \
  -e MACHINE_LEARNING_PRELOAD__CLIP__TEXTUAL=ViT-B-32__openai \
  -e MACHINE_LEARNING_PRELOAD__FACIAL_RECOGNITION__DETECTION=buffalo_l \
  -e MACHINE_LEARNING_PRELOAD__FACIAL_RECOGNITION__RECOGNITION=buffalo_l \
  -e MACHINE_LEARNING_PRELOAD__OCR__DETECTION=PP-OCRv5_mobile \
  -e MACHINE_LEARNING_PRELOAD__OCR__RECOGNITION=PP-OCRv5_mobile \
  -e MACHINE_LEARNING_PRELOAD__OCR__MAX_RESOLUTION=736 \
  ghcr.io/immich-app/immich-machine-learning:v3.3.6
```

The image already sets the model cache to `/cache`. The first start downloads the models. `GET http://127.0.0.1:3003/ping` returns `pong` after they have loaded.

Point `IMMICH_MACHINE_LEARNING_URL` at that machine from the Pi. Keep port 3003 on the local network. The Pi sends one preview at a time. On startup, and from the admin job buttons, it catches up on missing smart-search vectors, faces, and OCR.

## Build

Local binary, from this repo:

```sh
cargo build -p immich-server --release --locked
```

The binary is `target/release/immich-server`. Run it from a directory where `immich.env` has been loaded:

```sh
set -a
. ./immich.env
set +a
./target/release/immich-server
```

### Raspberry Pi Zero 2 W

Compile on another machine. Add the target once:

```sh
rustup target add aarch64-unknown-linux-gnu
```

Zig 0.17 rejects the target triple Cargo passes to the linker, so `cargo zigbuild` will fail. Save the following as `scripts/zig-aarch64-cc` and `scripts/zig-ar` and mark both executable.

`scripts/zig-aarch64-cc`:

```python
#!/usr/bin/env python3
import os, sys
args, skip = [], False
for arg in sys.argv[1:]:
    if skip:
        skip = False
        continue
    if arg in ("--target", "-target"):
        skip = True
        continue
    if arg.startswith("--target=") or arg.startswith("-target="):
        continue
    args.append(arg)
os.execvp("zig", ["zig", "cc", "-target", "aarch64-linux-gnu.2.36", *args])
```

`scripts/zig-ar`:

```python
#!/usr/bin/env python3
import os, sys
os.execvp("zig", ["zig", "ar", *sys.argv[1:]])
```

Then, from the repo root, with `zig` on `PATH`:

```sh
SKILL=scripts
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$PWD/$SKILL/zig-aarch64-cc"
export CC_aarch64_unknown_linux_gnu="$PWD/$SKILL/zig-aarch64-cc"
export AR_aarch64_unknown_linux_gnu="$PWD/$SKILL/zig-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_AR="$PWD/$SKILL/zig-ar"
cargo build -p immich-server --release --locked --target aarch64-unknown-linux-gnu
```

`file target/aarch64-unknown-linux-gnu/release/immich-server` should report an ARM aarch64 ELF. The newest `GLIBC_` symbol in the binary should be `GLIBC_2.36` or older.

## Install on the Pi

Stop the server before replacing the binary. In the files below, replace `USER` with the account that will run the server. Lay the files out as:

```text
/home/USER/immich/
  immich-server
  immich.env
  start.sh
  immich.log
  web/            # contents of the v3.3.6 web build
  bin/ffmpeg      # optional static ffmpeg, first on PATH
  data/
```

`start.sh`:

```sh
#!/bin/sh
set -a
. /home/USER/immich/immich.env
set +a
PATH="/home/USER/immich/bin:$PATH"
export PATH
cd /home/USER/immich
exec ./immich-server
```

`chmod 755` both `immich-server` and `start.sh`. Create an empty `immich.log`.

User systemd unit, `~/.config/systemd/user/immich.service`:

```ini
[Unit]
Description=Immich photo server

[Service]
Type=simple
WorkingDirectory=/home/USER/immich
ExecStart=/home/USER/immich/start.sh
Restart=on-failure
RestartSec=3
StandardOutput=append:/home/USER/immich/immich.log
StandardError=append:/home/USER/immich/immich.log

[Install]
WantedBy=default.target
```

Enable it for the same user that owns the files, and keep it running after logout:

```sh
loginctl enable-linger "$USER"
systemctl --user daemon-reload
systemctl --user enable --now immich
```

Copy a new binary with the service stopped, then start it again. Leave `data/` and `immich.env` in place. Leave `data/keys/aes-256.key` in place.

## Check

```sh
curl -fsS http://pizero.local:2283/api/server/ping
```

The body is `{"res":"pong"}`. The log contains `immich listening on 0.0.0.0:2283`.

In the mobile app, set the server endpoint to `http://<pi>:2283`. The same address opens the web UI in a browser.

## Backups

Copy `data/immich.db` and, when object storage is on, `data/keys/aes-256.key`. The app can also run a SQLite `VACUUM INTO` backup from the admin maintenance screen. A Postgres `.sql` dump does not load into this database.

`data/spool/` holds uploads that have been accepted and are still waiting to be sealed and sent. Leave it alone while the server is stopped. Startup finishes those files.

## Limits

Still photos are stored as JPEG thumbnails. HEIC originals do not decode. Video playback is the original file, with HTTP range requests, including after sealing. The server does not transcode video. The Pi Zero 2 W has no AES hardware, so sealing runs in software.

## Development

```sh
cargo test -p immich-server --offline -- --test-threads=8
```

Tests use a local data directory and do not need object storage or the machine-learning container.
