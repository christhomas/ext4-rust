# fs-ext4 Improvement Plan

Generated: 2026-04-19. Triaged list of critical gaps, bugs, and
performance wins for the pure-Rust ext4 driver at `v0.1.0`.

## Top 10 Priorities (by shipping impact)

1. **Journaled writes** — write path currently bypasses JBD2. Crash = metadata corruption.
2. **Depth ≥2 extent mutations** — files that would need a second leaf-block split fail loudly.
3. **Sparse-file growth via `truncate`** — `plan_truncate_grow` is a no-op beyond size bump.
4. **External xattr blocks (write)** — in-inode only; overflow returns `ENOSPC` instead of allocating an xattr block.
5. **Unwrap panics on parse** — ~250 `.try_into().unwrap()` in superblock/inode/extent/journal/dir parsing. Fuzzer-critical.
6. **64-bit overflow in block math** — `physical_block * block_size` in `file_io.rs:91`, `alloc.rs:160`. Checked arithmetic needed.
7. **No block cache / readahead** — every extent lookup hits the device. Sequential 1 MB read = hundreds of redundant seeks.
8. **Metadata checksum *generation*** — reads verify; writes don’t regenerate `et_checksum`, dir-tail, BGD csum. Corrupts `METADATA_CSUM` images.
9. **No fsck-like validation** — orphan inodes, link count audit, bitmap consistency. A half-applied write never gets swept.
10. **Error-code resolution** — generic `Corrupt` / `InvalidArgument` covers 20+ distinct cases; FFI callers can’t map to errno.

## Architecture (current)

| Layer | Module(s) | Status |
|---|---|---|
| Device | `block_io` | read + write, no cache |
| Superblock | `superblock`, `features`, `bgd` | read + validate |
| Inode | `inode` | read + extra timestamps |
| Extents | `extent`, `extent_mut` | read full tree; write up to depth 1 |
| Data | `file_io`, `file_mut`, `inline_data` | read all; write replace + shrink-truncate |
| Directory | `dir`, `htree`, `htree_mut` | read all; write linear + htree |
| Xattr | `xattr`, `acl`, `ea_inode` | read all; write in-inode only |
| Journal | `jbd2`, `journal`, `journal_apply`, `transaction` | replay at mount; live writes unjournaled |
| FFI | `capi`, `include/fs_ext4.h` | stable `fs_ext4_*` |

## Detailed Plan

### Phase A — Stability (no behavior change; fewer panics, better errors)

**A1. Purge `.unwrap()` from parse paths.** Target modules: `dir.rs`,
`inode.rs`, `extent.rs`, `extent_mut.rs`, `superblock.rs`, `bgd.rs`,
`journal.rs`, `acl.rs`, `xattr.rs`. Pattern — convert
`slice.try_into().unwrap()` into `slice.try_into().map_err(|_|
Error::Corrupt("…"))?` with a distinct message per site.

**A2. Checked arithmetic.** Hot sites:
- `file_io.rs` — physical block → byte offset.
- `alloc.rs` — inode table offset and block-within-group math.
- `extent.rs` — logical→physical projection for uninitialized extents.

**A3. FFI input validation.** `capi.rs`:
- Xattr names: reject embedded NUL and empty names.
- Paths: UTF-8 validated, but reject empty paths before lookup.
- Reject extremely long names (>255) at the ABI boundary instead of deep in `dir.rs`.

**A4. Richer `Error` variants.** Split `Corrupt` into:
`CorruptSuperblock`, `CorruptBgd`, `CorruptInode`, `CorruptExtentTree`,
`CorruptDir`, `CorruptXattr`, `CorruptJournal`. Keep the `&'static str`
context payload; update `error::to_errno()` → all still map to `EIO`,
but the `last_error()` string is now actionable.

### Phase B — Correctness (catch silent data loss)

**B1. Metadata-csum generation.** Wire `Checksummer::patch_extent_tail`
everywhere a leaf block is mutated. Add symmetric helpers:
- `patch_dir_block_tail(buf, sb_seed, inode_csum_seed)` — already
  partially present; ensure called on every dir block flush in
  `htree_mut.rs` + `dir_mut`.
- `patch_bgd_checksum(bgd_bytes, sb)` — call after bitmap writes in `alloc.rs`.
- `patch_inode_checksum(inode_bytes, sb, inode_num)` — call on every
  inode flush (create, unlink, chmod, chown, utimens, truncate, write).

**B2. Inode link count audit.** New `Filesystem::verify_link_counts` —
walks every directory, counts refs to each inode, compares against
`i_links_count`. Expose via FFI as optional `fs_ext4_fsck_audit()`
returning a list of anomalies. Read-only.

**B3. Sparse-file growth.** `plan_truncate_grow`:
- Compute `(old_size, new_size)` in blocks.
- Emit `Extent { ee_start=0, ee_len=block_count, uninitialized=true }`
  records filling the gap. Write inserts honour `EXT4_EXT_UNWRITTEN_MASK`.
- Reads of uninitialized extents already return zeros.

### Phase C — Performance

**C1. LRU block cache.** In `block_io.rs`:
- Wrap `BlockDevice` reads with a small LRU (default 64 × 4 KiB).
- Invalidate on writes. Skip on `mount_with_callbacks` unless opted in.
- Measurable win on extent-heavy workloads (manyfiles, largedir).

**C2. Extent lookup memoization.** In `file_io::read`, cache the last
extent hit per open file; re-traverse only on miss.

**C3. Bitmap scan vectorization.** `alloc.rs:find_free_bit` — step by
`u64` (`from_le_bytes`), use `trailing_ones` to skip whole words. 8-16×
faster on sparse bitmaps.

### Phase D — Testing

**D1. Malformed-image negative tests.** `tests/fuzz_smoke.rs`:
- Truncate a valid image to 1 KiB — `Filesystem::mount` must return
  `Err`, not panic.
- Flip random bytes in the superblock — same invariant.
- Zero-fill an inode table — directory walk must return `Err`.

**D2. Sparse-growth integration test.** Build on `capi_truncate_grow.rs`:
grow a 4 KiB file to 1 MiB, read back — first 4 KiB is original, rest is zeros.

**D3. Checksum regeneration.** `tests/capi_write_file_csum_integrity.rs`
already exists; extend with a post-write `e2fsck -fn` invocation (when
`e2fsck` is on `$PATH`, skip otherwise) to confirm no warnings.

### Phase E — Documentation

**E1.** `docs/IMPROVEMENT-PLAN.md` (this file).
**E2.** `docs/WRITE-PATH.md` — how a write travels from `capi` to disk,
what’s journaled (nothing yet), ordering guarantees (none yet).
**E3.** `docs/SAFETY.md` — FFI contract, thread-safety, lifetime rules.

## Non-goals / Deferred

- **Encryption (`ext4_encrypt`)** — large surface; no consumer demand.
- **Casefold** — stub exists; no consumer demand.
- **`quota`, `verity`, `project_quota`** — out of scope.
- **Inline directory data** — rare; ext4 default is extent-based dirs.

## Session plan (autonomous pass)

Working through: A1 → A2 → A3 → A4 → B1 → C1 → C2 → D1 → D2 → B3.
One commit per logical step. Bail-out rule: if the test pass rate drops
and a rollback can’t recover, stop and leave a `WIP:` commit for review.
