# Plan: ZFS Support for bsdeploy

## Objective
Leverage FreeBSD's ZFS filesystem to make image creation and jail cloning near-instant and extremely storage-efficient.

## Proposed Changes

### 1. ZFS Dataset Structure
We will organize datasets to allow for easy snapshots and cloning:
- `zroot/bsdeploy`: Base dataset.
- `zroot/bsdeploy/base/<version>`: Extracted FreeBSD base system.
- `zroot/bsdeploy/images/<hash>`: Captured build images.
- `zroot/bsdeploy/jails/<name>`: Clones of images for running jails.

### 2. Workflow Transition

#### Image Creation (Build)
1.  Create a ZFS dataset for the image: `zfs create zroot/bsdeploy/images/<hash>`.
2.  Perform the build inside a temporary jail (as before).
3.  Instead of `rsync` to a directory, we move the files into the new ZFS dataset.
4.  **Snapshot the image**: `zfs snapshot zroot/bsdeploy/images/<hash>@base`.

#### Jail Creation (Deploy)
1.  **Clone the snapshot**: `zfs clone zroot/bsdeploy/images/<hash>@base zroot/bsdeploy/jails/<name>`.
    - This is near-instant.
    - It uses **zero additional space** initially.
2.  Mount the standard RO layers (`/bin`, `/lib`, etc.) from the Base dataset via `nullfs` (as before).

### 3. Implementation Steps

#### Phase 1: Detection & Setup
- Add `remote::get_zfs_pool(path)` helper.
- Update `Setup` command to offer creating the `bsdeploy` dataset if ZFS is detected.

#### Phase 2: ZFS Primitives
- Implement `jail::create_zfs()` and `image::capture_zfs()`.
- Ensure cleanup logic handles `zfs destroy`.

#### Phase 3: Hybrid Support
- `bsdeploy` will check at runtime:
    - If `/usr/local/bsdeploy` is on ZFS -> Use ZFS logic.
    - Otherwise -> Fallback to current `rsync` + `mkdir` logic.

## Advantages
- **Speed**: Deployments become significantly faster as gigabytes of data are "cloned" in milliseconds.
- **Space**: No more `No space left on device` errors from duplicated `/var` or `/usr/local` files.
- **Atomic Rollbacks**: `zfs rollback` allows instant reversion if a jail becomes corrupted.
