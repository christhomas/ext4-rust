#!/bin/bash
# Build the ext4 test-disk fixtures inside a qemu-hosted Alpine Linux VM.
#
# Why qemu: mkfs.ext4, loop-mount, setfattr, setfacl — all Linux-only.
# qemu works everywhere (macOS, Linux, in CI), so one script drives
# the build on any host. Nothing about ext4rs itself touches platform
# specifics; this is just a build-time convenience.
#
# First run downloads Alpine's netboot kernel + initramfs + modloop
# (~40 MB total) into .vm-cache/. Subsequent runs reuse the cache.
#
# Usage:
#   bash build-ext4-feature-images.sh              # build all images
#   bash build-ext4-feature-images.sh htree xattr  # build named ones
#
# Requires: qemu-system-x86_64, python3 (for the tiny apkovl HTTP
# server), tar, curl. All available on macOS (brew install qemu),
# ubuntu-latest (apt install qemu-system-x86), and alpine/fedora.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

CACHE="$SCRIPT_DIR/.vm-cache"
mkdir -p "$CACHE"

# ---------------------------------------------------------------------------
# Step 1 — pin Alpine version + download netboot assets on first run.
# ---------------------------------------------------------------------------
ALPINE_VER=3.21.4
ALPINE_REL="${ALPINE_VER%.*}"
ALPINE_BASE="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_REL}/releases/x86_64/netboot-${ALPINE_VER}"

download_if_missing() {
    local url="$1" out="$2"
    if [ ! -s "$out" ]; then
        echo "[host] downloading $(basename "$out")..."
        curl -fsSL -o "$out" "$url"
    fi
}
download_if_missing "$ALPINE_BASE/vmlinuz-virt"   "$CACHE/vmlinuz-virt"
download_if_missing "$ALPINE_BASE/initramfs-virt" "$CACHE/initramfs-virt"
download_if_missing "$ALPINE_BASE/modloop-virt"   "$CACHE/modloop-virt"

# ---------------------------------------------------------------------------
# Step 2 — assemble the apkovl (Alpine overlay) that wires our guest
# builder in as an auto-started local.d service.
# ---------------------------------------------------------------------------
OVL_TMP="$CACHE/ovl"
rm -rf "$OVL_TMP"
mkdir -p "$OVL_TMP/etc/local.d" "$OVL_TMP/etc/runlevels/default" "$OVL_TMP/etc/apk"

# /etc/apk/world — packages Alpine's diskless setup will install
# during boot (via `apk add` after applying the overlay). These are
# everything we need to run the mkfs/mount/xattr/acl commands.
cat > "$OVL_TMP/etc/apk/world" <<'PKGS_EOF'
alpine-base
busybox
e2fsprogs
e2fsprogs-extra
attr
acl
util-linux
PKGS_EOF

# Use HTTP (not HTTPS) for the apk mirror — qemu NAT DNS works,
# but TLS handshake against dl-cdn has been flaky in the boot
# environment. APK packages are signed independently of TLS.
cat > "$OVL_TMP/etc/apk/repositories" <<REPO_EOF
http://dl-cdn.alpinelinux.org/alpine/v${ALPINE_REL}/main
http://dl-cdn.alpinelinux.org/alpine/v${ALPINE_REL}/community
REPO_EOF

# qemu's built-in DNS at 10.0.2.3 works but the guest's resolver
# doesn't always detect it; pin public resolvers explicitly.
cat > "$OVL_TMP/etc/resolv.conf" <<'DNS_EOF'
nameserver 10.0.2.3
nameserver 1.1.1.1
nameserver 8.8.8.8
DNS_EOF

# Wrapper that chains the real builder (which lives on the 9p host
# share, so we don't bake it into the apkovl). Writes a done-marker
# back to the host so the watchdog knows the guest finished cleanly.
cat > "$OVL_TMP/etc/local.d/99-ext4.start" <<'WRAPPER_EOF'
#!/bin/sh
set -eu
echo "[vm] local.d starting"
# modules we need for 9p
modprobe 9p 9pnet 9pnet_virtio 2>/dev/null || true
modprobe loop 2>/dev/null || true

mkdir -p /host
mount -t 9p -o trans=virtio,version=9p2000.L,msize=131072 host /host

sh /host/_vm-builder.sh $(cat /host/.vm-cache/vm-args 2>/dev/null) \
        > /host/.vm-cache/vm-build.log 2>&1 \
    && touch /host/.vm-cache/vm-build.done \
    || touch /host/.vm-cache/vm-build.failed

sync
poweroff -f
WRAPPER_EOF
chmod +x "$OVL_TMP/etc/local.d/99-ext4.start"

# Enable the `local` service so OpenRC runs /etc/local.d/*.start at boot.
ln -sf /etc/init.d/local "$OVL_TMP/etc/runlevels/default/local"

# Apkovl must be a .tar.gz whose filename matches a hostname the
# overlay applies to. Alpine init also accepts a generic filename
# via the `apkovl=` boot option, which is what we use below.
rm -f "$CACHE/ovl.apkovl.tar.gz" "$CACHE/vm-build.done" "$CACHE/vm-build.failed" "$CACHE/vm-build.log"
(cd "$OVL_TMP" && tar -czf "$CACHE/ovl.apkovl.tar.gz" etc)

# ---------------------------------------------------------------------------
# Step 3 — serve the apkovl over a throwaway localhost HTTP server so
# Alpine's init can fetch it via qemu's NAT gateway (10.0.2.2).
# ---------------------------------------------------------------------------
HTTP_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
python3 -m http.server "$HTTP_PORT" --directory "$CACHE" --bind 127.0.0.1 >/dev/null 2>&1 &
HTTP_PID=$!
sleep 0.3
cleanup() {
    kill "$HTTP_PID" 2>/dev/null || true
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Step 4 — boot Alpine under qemu with a 9p share of this directory.
# ---------------------------------------------------------------------------
echo "[host] booting Alpine under qemu (serial -> stdout)..."

# Pass the requested image-name list through to the guest by storing
# it on the 9p share — the local.d wrapper reads it back.
printf '%s\n' "$@" > "$CACHE/vm-args"

qemu-system-x86_64 \
    -kernel "$CACHE/vmlinuz-virt" \
    -initrd "$CACHE/initramfs-virt" \
    -append "console=ttyS0 modules=loop,squashfs,sd-mod,virtio_blk,virtio_net,virtio_pci,9p,9pnet_virtio ip=dhcp apkovl=http://10.0.2.2:${HTTP_PORT}/ovl.apkovl.tar.gz alpine_repo=http://dl-cdn.alpinelinux.org/alpine/v${ALPINE_REL}/main modloop=http://10.0.2.2:${HTTP_PORT}/modloop-virt" \
    -virtfs local,path="$SCRIPT_DIR",mount_tag=host,security_model=mapped-xattr,id=host \
    -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
    -m 1024 \
    -smp 2 \
    -nographic \
    -no-reboot

# ---------------------------------------------------------------------------
# Step 5 — inspect the done-marker the guest left behind.
# ---------------------------------------------------------------------------
if [ -f "$CACHE/vm-build.done" ]; then
    echo "[host] guest reported success."
    exit 0
elif [ -f "$CACHE/vm-build.failed" ]; then
    echo "[host] guest reported failure. Last 50 lines of vm-build.log:" >&2
    tail -n 50 "$CACHE/vm-build.log" >&2 || true
    exit 1
else
    echo "[host] guest exited without writing a done marker — something" >&2
    echo "       went wrong during boot. Check earlier serial output." >&2
    exit 1
fi
