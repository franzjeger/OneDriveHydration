# Sync correctness gate

The desktop shell is not the release criterion. A build is a usable OneDrive
client only when the namespace and content operations below pass against a
dedicated, non-production Microsoft 365 tenant and survive a daemon restart.

Do not run this matrix in a personal or production drive. Create a unique test
folder, record its drive and item IDs, and remove it through the OneDrive web UI
after the run. The client must never be given the tenant root as the disposable
test scope.

## Current contract

This table describes the HydrationAPI revision pinned by this product. Automated
rows still require their corresponding live result before release.

| Operation | Automated seam | Live Graph | Release state |
|---|---:|---:|---|
| Remote file create/update/delete | Yes | Required | Candidate |
| Remote file rename or parent move | Yes | Required | Candidate |
| Whole-file hydration with cTag version and QuickXor integrity | Yes | Required | Candidate |
| Local file create | Yes | Required | Candidate |
| Local in-place edit | Yes | Required | Candidate |
| Local atomic-save replacement | Yes | Required | Candidate |
| Local file delete with recorded cTag | Yes | Required | Candidate |
| Local same-folder file rename | Yes | Required | Candidate |
| Local file move to another folder | Yes | Required | Candidate |
| Empty folder in either direction | Yes | Required | Candidate |
| Local folder create/rename/move/delete | Yes | Required | Candidate |
| Two-device edit, rename and delete conflicts | Partial | Required | Blocking |
| Restart during fetch/upload/delta apply | Partial | Required | Blocking |

`Candidate` does not mean released. It means the operation has a fail-closed
implementation and adversarial tests, and is ready for the live row. `Blocking`
means the product must say that it is unsupported; a tray or installer cannot
turn it into a supported operation.

## Automated gate

Run HydrationAPI at the exact revision intended for the product:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --no-fail-fast
cargo doc --workspace --all-features --no-deps
```

Then update the revision in this repository and run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --no-fail-fast
cargo doc --workspace --all-features --no-deps
```

The pinned HydrationAPI revision is part of the result. Testing a local checkout
while shipping a different Git revision does not satisfy the gate.

## Live matrix

For every row, capture daemon logs, the local relative path, Graph drive/item
ID, cTag, size and QuickXorHash before and after. Wait for both directions to
settle, restart both daemon halves, and verify the result again.

1. Create a small file, a multi-fragment file and a zero-filled file remotely.
   Read the first and last ranges locally, then read the full files and compare
   hashes.
2. Edit a hydrated file locally in place. Verify that Graph kept the item ID,
   changed the cTag and received byte-identical content.
3. Save the same file through an editor-style temporary-file rename. Verify the
   same object was updated rather than a second object being created or a 409
   being retried forever.
4. Rename a file inside one folder. Verify one conditional `PATCH` by item ID,
   no content upload, and no reversal by the next delta pass.
5. Delete both a hydrated file and an untouched placeholder locally. Verify the
   objects enter the recycle bin. Repeat after changing one remotely first; the
   stale conditional delete must fail and retain the newer remote work.
6. Create, edit, rename, move and delete files remotely. Verify each local
   result, including after restart from the last committed delta cursor.
7. Interrupt the daemon during metadata fetch, content download, upload
   fragment, final upload response, local placement and delta commit. Every
   restart must converge without advancing past unapplied work.
8. Run two clients against the same test folder. Exercise edit/edit,
   rename/edit, delete/edit and rename/rename. No path may silently overwrite or
   delete a version neither client observed.
9. Trigger throttling and transient 5xx responses. Pending work must remain
   visible, retry with bounded backoff, and retain every precondition.
10. Move or rename a folder and create an empty folder in each direction.
     Verify that the folder identity follows the rename, that the empty folder
     appears on the other side, and that deleting a non-empty folder on one
     side retains the local (and cloud) content rather than erasing it.

## Release rule

The client remains **not ready for user data** until every blocking namespace
row has an explicit contract, adversarial automated coverage and a passing live
result. Product-shell work stays frozen except where needed to expose an honest
sync error or unsupported operation.
