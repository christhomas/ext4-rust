#!/bin/sh
# GUEST-side: runs inside the qemu Alpine VM. Installs e2fsprogs,
# mounts the host share, and builds every ext4 feature fixture
# by running mkfs.ext4 + loop-mount + file-populate for each one.
#
# The host share is 9p-mounted at /host and contains the full
# test-disks/ directory. Images are written back to /host so they
# land directly on the host filesystem.

set -eu

echo "[vm] installing e2fsprogs + attr + acl..."
apk add --no-cache e2fsprogs attr acl >/dev/null

mkdir -p /host
mount -t 9p -o trans=virtio,version=9p2000.L,msize=131072 host /host
cd /host

# --- image builders -------------------------------------------------------

build_basic() {
    local img=ext4-basic.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum -L testvolume $img
    mkdir -p /mnt/img && mount -o loop $img /mnt/img
    printf 'hello from ext4\n' > /mnt/img/test.txt
    mkdir -p /mnt/img/subdir
    ln -s test.txt /mnt/img/link.txt
    sync
    umount /mnt/img
}

build_htree() {
    local img=ext4-htree.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum -L htree-vol $img
    mount -o loop $img /mnt/img
    mkdir -p /mnt/img/bigdir
    i=1
    while [ $i -le 256 ]; do
        printf 'content of file %03d\n' $i > /mnt/img/bigdir/file_$i.txt
        i=$((i + 1))
    done
    echo 'small file content' > /mnt/img/small.txt
    sync
    umount /mnt/img
}

build_csum_seed() {
    local img=ext4-csum-seed.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,extent,64bit,flex_bg,metadata_csum,metadata_csum_seed -L csum-seed-vol $img
    mount -o loop $img /mnt/img
    echo 'pi-style file' > /mnt/img/hello.txt
    mkdir -p /mnt/img/etc
    echo 'fake fstab' > /mnt/img/etc/fstab
    sync
    umount /mnt/img
}

build_no_csum() {
    local img=ext4-no-csum.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super -L no-csum-vol $img
    mount -o loop $img /mnt/img
    echo 'plain file' > /mnt/img/hello.txt
    sync
    umount /mnt/img
}

build_deep_extents() {
    local img=ext4-deep-extents.img
    echo "[vm] $img"
    rm -f $img
    # Needs to be large enough to hold a file with many non-contiguous extents.
    truncate -s 64M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum -L deep-vol $img
    mount -o loop $img /mnt/img
    # Produce a fragmented file by writing lots of small 4K-aligned
    # holes interleaved with data.
    dd if=/dev/urandom of=/mnt/img/fragmented.bin bs=4K count=5000 status=none
    sync
    umount /mnt/img
}

build_inline() {
    local img=ext4-inline.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum,inline_data -L inline-vol $img
    mount -o loop $img /mnt/img
    printf 'tiny' > /mnt/img/tiny.txt
    mkdir -p /mnt/img/inline_dir
    sync
    umount /mnt/img
}

build_xattr() {
    local img=ext4-xattr.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum -L xattr-vol $img
    mount -o loop,user_xattr $img /mnt/img
    echo 'has xattrs' > /mnt/img/file.txt
    setfattr -n user.comment -v 'hello-xattr' /mnt/img/file.txt
    setfattr -n user.mood -v 'cheery' /mnt/img/file.txt
    sync
    umount /mnt/img
}

build_acl() {
    local img=ext4-acl.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum -L acl-vol $img
    mount -o loop,acl $img /mnt/img
    echo 'has ACLs' > /mnt/img/file.txt
    setfacl -m u:1001:rwx /mnt/img/file.txt
    setfacl -m g:500:rx /mnt/img/file.txt
    sync
    umount /mnt/img
}

build_largedir() {
    local img=ext4-largedir.img
    echo "[vm] $img"
    rm -f $img
    # largedir feature allows directories > 2GB; a modest 32M image
    # is enough to exercise the on-disk htree depth bump the driver
    # cares about.
    truncate -s 32M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum,large_dir -L largedir-vol $img
    mount -o loop $img /mnt/img
    mkdir -p /mnt/img/hugedir
    i=1
    while [ $i -le 4096 ]; do
        : > /mnt/img/hugedir/file_$i.txt
        i=$((i + 1))
    done
    sync
    umount /mnt/img
}

build_manyfiles() {
    local img=ext4-manyfiles.img
    echo "[vm] $img"
    rm -f $img
    truncate -s 16M $img
    mkfs.ext4 -q -F -O has_journal,ext_attr,dir_index,filetype,extent,64bit,flex_bg,sparse_super,metadata_csum -L many-vol $img
    mount -o loop $img /mnt/img
    i=1
    while [ $i -le 512 ]; do
        printf 'f%04d\n' $i > /mnt/img/file_$i.txt
        i=$((i + 1))
    done
    sync
    umount /mnt/img
}

mkdir -p /mnt/img

if [ $# -eq 0 ]; then
    build_basic
    build_htree
    build_csum_seed
    build_no_csum
    build_deep_extents
    build_inline
    build_xattr
    build_acl
    build_largedir
    build_manyfiles
else
    for name in "$@"; do
        "build_$name"
    done
fi

echo "[vm] done — syncing + powering off."
sync
poweroff -f
