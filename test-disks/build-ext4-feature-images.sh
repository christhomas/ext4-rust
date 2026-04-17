#!/bin/bash
# Build a matrix of ext4 test images covering different feature combinations.
# Each image gets a sibling .meta.txt describing its features + expected contents.
#
# Why Docker: macOS doesn't have mkfs.ext4. We use a tiny Linux container
# (debian:bookworm-slim has e2fsprogs in ~50MB) to build the images.
#
# Usage: bash build-ext4-feature-images.sh [name1 name2 ...]
#        (no args = build all)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if ! command -v docker >/dev/null; then
    echo "ERROR: docker required (macOS lacks mkfs.ext4)" >&2
    exit 1
fi

IMAGE=debian:bookworm-slim

# Helper: run a shell snippet inside the container with /work mounted to here.
# --privileged needed for `mount -o loop` inside the container.
in_container() {
    docker run --rm --platform=linux/amd64 --privileged \
        -v "$SCRIPT_DIR:/work" -w /work \
        "$IMAGE" bash -c "apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq e2fsprogs attr acl >/dev/null 2>&1 && $1"
}

build_htree() {
    local img=ext4-htree.img
    local meta=ext4-htree.meta.txt
    echo "==> $img — directory with many entries forcing htree indexing"
    in_container "
        rm -f $img
        truncate -s 16M $img
        mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,large_file,huge_file,uninit_bg,metadata_csum -L htree-vol $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        # Create 256 files in /bigdir to force htree indexing
        mkdir -p /mnt/img/bigdir
        for i in \$(seq 1 256); do
            printf 'content of file %03d\n' \$i > /mnt/img/bigdir/file_\$i.txt
        done
        echo 'small file content' > /mnt/img/small.txt
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,large_file,huge_file,uninit_bg,metadata_csum
volume_label: htree-vol
contents:
  /small.txt — "small file content\n" (19 bytes)
  /bigdir/   — 256 files (file_001.txt .. file_256.txt), forces htree indexing
test_targets:
  - lookup any /bigdir/file_NNN.txt via htree path
  - readdir of /bigdir returns 258 entries (256 + . + ..)
EOF
}

build_csum_seed() {
    local img=ext4-csum-seed.img
    local meta=ext4-csum-seed.meta.txt
    echo "==> $img — Pi SD card style with INCOMPAT_CSUM_SEED"
    in_container "
        rm -f $img
        truncate -s 16M $img
        # csum_seed requires metadata_csum
        mkfs.ext4 -q -F -O has_journal,extent,64bit,flex_bg,metadata_csum,metadata_csum_seed -L csum-seed-vol $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        echo 'pi-style file' > /mnt/img/hello.txt
        mkdir -p /mnt/img/etc
        echo 'fake fstab' > /mnt/img/etc/fstab
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: has_journal,extent,64bit,flex_bg,metadata_csum,metadata_csum_seed
volume_label: csum-seed-vol
contents:
  /hello.txt   — "pi-style file\n" (14 bytes)
  /etc/fstab   — "fake fstab\n" (11 bytes)
notes:
  INCOMPAT_CSUM_SEED stores the checksum seed in superblock instead of
  deriving from UUID. Same flag that broke lwext4 on the Pi SD card.
EOF
}

build_no_csum() {
    local img=ext4-no-csum.img
    local meta=ext4-no-csum.meta.txt
    echo "==> $img — legacy ext4 without metadata_csum"
    in_container "
        rm -f $img
        truncate -s 8M $img
        mkfs.ext4 -q -F -O ^metadata_csum,extent,64bit,filetype,dir_index,sparse_super -L no-csum-vol $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        echo 'no checksum here' > /mnt/img/file.txt
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: extent,64bit,filetype,dir_index,sparse_super (NO metadata_csum)
volume_label: no-csum-vol
contents:
  /file.txt — "no checksum here\n" (17 bytes)
notes:
  Tests that we don't accidentally require checksums when the FS lacks them.
EOF
}

build_deep_extents() {
    local img=ext4-deep-extents.img
    local meta=ext4-deep-extents.meta.txt
    echo "==> $img — file with multi-level extent tree (sparse + fragmented)"
    in_container "
        rm -f $img
        truncate -s 64M $img
        mkfs.ext4 -q -F -O extent,64bit,flex_bg,metadata_csum -L deep-vol $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        # Create a sparse file with many holes to force extent tree depth.
        # Write 1 byte at offsets 0, 64K, 128K, ... 16M = ~256 extents,
        # which should overflow the 4-extent inline limit and require leaf blocks.
        dd if=/dev/zero of=/mnt/img/sparse.bin bs=1 count=0 seek=16M status=none
        for off in \$(seq 0 65536 16000000); do
            printf 'X' | dd of=/mnt/img/sparse.bin bs=1 count=1 seek=\$off conv=notrunc status=none
        done
        # Also a small dense file as control
        echo 'control file' > /mnt/img/dense.txt
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: extent,64bit,flex_bg,metadata_csum
volume_label: deep-vol
contents:
  /sparse.bin  — 16 MB sparse file with single 'X' bytes every 64 KB
                 (~245 extents — forces multi-level extent tree)
  /dense.txt   — "control file\n" (13 bytes, single inline extent)
test_targets:
  - extent::lookup must descend through internal nodes for high logical blocks
  - sparse holes should read as zero
EOF
}

build_inline() {
    local img=ext4-inline.img
    local meta=ext4-inline.meta.txt
    echo "==> $img — files using INCOMPAT_INLINE_DATA (data lives in inode)"
    in_container "
        rm -f $img
        truncate -s 8M $img
        # inline_data must be set at mkfs time
        mkfs.ext4 -q -F -O ext_attr,extent,64bit,filetype,dir_index,metadata_csum,inline_data -L inline-vol -I 256 $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        # Tiny file (≤60 bytes) — fits in i_block alone
        echo 'tiny inline' > /mnt/img/tiny.txt
        # Medium file (60–~150 bytes) — overflows into system.data xattr
        printf 'A%.0s' \$(seq 1 100) > /mnt/img/medium.txt
        # Symlink (always inline if ≤60 bytes)
        ln -s 'target/path/here' /mnt/img/symlink
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: ext_attr,extent,64bit,filetype,dir_index,metadata_csum,inline_data
volume_label: inline-vol
contents:
  /tiny.txt    — "tiny inline\n" (12 bytes, in i_block only)
  /medium.txt  — 100x 'A' (100 bytes, overflows to system.data xattr)
  /symlink     — symlink to "target/path/here"
test_targets:
  - read /tiny.txt returns full 12 bytes
  - read /medium.txt returns full 100 bytes (i_block + xattr concat)
  - readlink /symlink returns "target/path/here"
EOF
}

build_xattr() {
    local img=ext4-xattr.img
    local meta=ext4-xattr.meta.txt
    echo "==> $img — files with extended attributes (Finder-style metadata)"
    in_container "
        rm -f $img
        truncate -s 8M $img
        mkfs.ext4 -q -F -O ext_attr,extent,64bit,filetype,dir_index,metadata_csum,inline_data -L xattr-vol $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        echo 'has xattrs' > /mnt/img/tagged.txt
        # Set a few xattrs covering different namespaces
        setfattr -n user.color -v 'red' /mnt/img/tagged.txt
        setfattr -n user.com.apple.FinderInfo -v '0xDEADBEEF' /mnt/img/tagged.txt
        # Also a directory with xattrs
        mkdir /mnt/img/tagged_dir
        setfattr -n user.purpose -v 'documents' /mnt/img/tagged_dir
        # And a plain file with no xattrs as a control
        echo 'no xattrs here' > /mnt/img/plain.txt
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: ext_attr,extent,64bit,filetype,dir_index,metadata_csum,inline_data
volume_label: xattr-vol
contents:
  /tagged.txt    — "has xattrs\n"; xattrs: user.color=red, user.com.apple.FinderInfo=0xDEADBEEF
  /tagged_dir/   — directory; xattrs: user.purpose=documents
  /plain.txt     — "no xattrs here\n"; no xattrs
test_targets:
  - read user.color, user.com.apple.FinderInfo on /tagged.txt
  - read user.purpose on /tagged_dir
  - /plain.txt has empty xattr list
EOF
}

build_acl() {
    local img=ext4-acl.img
    local meta=ext4-acl.meta.txt
    echo "==> $img — files + dirs with POSIX ACL xattrs"
    in_container "
        set -e
        rm -f $img
        truncate -s 8M $img
        mkfs.ext4 -q -F -O ext_attr,extent,64bit,filetype,dir_index,metadata_csum -L acl-vol $img
        tune2fs -o acl,user_xattr $img
        mkdir -p /mnt/img && mount -o loop,acl,user_xattr $img /mnt/img
        echo 'minimal acl' > /mnt/img/mode_only.txt
        setfacl -m u::rwx,g::r-x,o::r-- /mnt/img/mode_only.txt
        echo 'named entries' > /mnt/img/named.txt
        setfacl -m u:1000:rw-,g:2000:r--,m::rwx /mnt/img/named.txt
        mkdir /mnt/img/acl_dir
        setfacl -m u::rwx,g::r-x,o::--x,d:u::rwx,d:g::r-x,d:o::--- /mnt/img/acl_dir
        echo 'no acl' > /mnt/img/plain.txt
        sync
        echo '--- verify ACL xattrs via getfattr (in container) ---'
        getfattr -m '^system.posix_acl' -d /mnt/img/named.txt /mnt/img/acl_dir || true
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: ext_attr,extent,64bit,filetype,dir_index,metadata_csum
volume_label: acl-vol
contents:
  /mode_only.txt  — access ACL: u::rwx, g::r-x, o::r--
  /named.txt      — access ACL: u::rwx, u:1000:rw-, g::r-x, g:2000:r--, m::rwx, o::r--
  /acl_dir/       — access + default ACL (u::rwx g::r-x o::--x; d:u::rwx d:g::r-x d:o::---)
  /plain.txt      — no ACL
test_targets:
  - system.posix_acl_access on /mode_only.txt decodes to 3 short entries
  - system.posix_acl_access on /named.txt has User(1000) + Group(2000) entries
  - /acl_dir has both system.posix_acl_access and system.posix_acl_default
  - /plain.txt has neither acl xattr
EOF
}

build_largedir() {
    local img=ext4-largedir.img
    local meta=ext4-largedir.meta.txt
    echo "==> $img — huge directory forcing deep htree (LARGEDIR ro_compat)"
    in_container "
        set -e
        rm -f $img
        # 192M gives enough room for 70k inodes (default 16K ratio => ~12k
        # inodes otherwise). -N overrides the inode count directly.
        truncate -s 192M $img
        mkfs.ext4 -q -F -N 80000 \
            -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,large_file,huge_file,uninit_bg,metadata_csum,large_dir \
            -L largedir-vol $img
        mkdir -p /mnt/img && mount -o loop $img /mnt/img
        mkdir -p /mnt/img/huge
        # Zero-length files via touch — dir entries are what we care about.
        # 70000 entries comfortably forces an htree >= 2 leaf levels with
        # LARGEDIR enabled (kernel lifts the 2-level cap when ro_compat
        # LARGEDIR is set).
        seq -w 1 70000 | while read -r i; do
            : > /mnt/img/huge/file_\$i.txt
        done
        echo 'control' > /mnt/img/small.txt
        sync
        umount /mnt/img
        chown $(id -u):$(id -g) $img
    "
    cat > "$meta" <<EOF
image: $img
features: has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,large_file,huge_file,uninit_bg,metadata_csum,large_dir
volume_label: largedir-vol
contents:
  /small.txt — "control\n" (8 bytes)
  /huge/     — 70000 zero-length files (file_00001.txt .. file_70000.txt)
               forces htree past its legacy 2-level cap (LARGEDIR ro_compat).
test_targets:
  - htree::lookup_leaf descends > 1 internal level to resolve /huge/file_NNNNN
  - readdir of /huge returns 70002 entries (70000 + . + ..)
  - random-sample stat on file_00001 / file_35000 / file_70000 all succeed
EOF
}

# Default: build all
ALL=(htree csum_seed no_csum deep_extents xattr inline acl largedir)
TARGETS=("${@:-${ALL[@]}}")

for t in "${TARGETS[@]}"; do
    case "$t" in
        htree)        build_htree ;;
        csum_seed)    build_csum_seed ;;
        no_csum)      build_no_csum ;;
        deep_extents) build_deep_extents ;;
        xattr)        build_xattr ;;
        inline)       build_inline ;;
        acl)          build_acl ;;
        largedir)     build_largedir ;;
        *)            echo "Unknown target: $t (have: ${ALL[*]})" >&2; exit 1 ;;
    esac
done

echo ""
echo "Done. Generated images:"
ls -lh ext4-*.img 2>/dev/null
