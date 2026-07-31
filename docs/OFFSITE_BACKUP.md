# Off-site recovery

Kosh can continuously copy its SQLite history and immutable attachment media
to a private Cloudflare R2 bucket, then publish complete recovery points. This
is disaster recovery for one writing installation. It is not multi-device
sync, collaboration, or a replacement for retaining the original Mac until a
recovery drill has passed.

Capture, editing, Exact search, and local citations never wait for R2,
Litestream, or the network.

## Set up the R2 target

Create a private, Standard-storage R2 bucket. Create an S3-compatible token
with **Object Read & Write** permission restricted to that one bucket. Kosh
does not need account-wide administration, public bucket access, or a custom
domain.

In Kosh, open Settings → Offsite recovery and enter:

- the 32-character Cloudflare account ID from the R2 endpoint;
- the private bucket name and its matching jurisdiction;
- the 32-character access key ID and 64-character secret access key;
- no backup set ID for a new library.

Use **Test connection**, then **Save target off**. Kosh stores the credential
pair under service `com.rohan.kosh.offsite-backup.r2` in macOS Keychain. It
stores only non-secret target and status data in SQLite and never returns
credentials to the webview or writes them to logs.

Record the generated backup set ID in a password manager or another location
outside Kosh. A clean Mac needs that ID, the R2 target, and the credential pair
to discover the correct recovery points.

Turn on **Back up this library**, then use **Back up now** for the first
complete point. A usable point exists only when all three Settings indicators
are healthy:

1. relational history has reached the exact remote transaction;
2. every referenced media object is present and verified;
3. the immutable checkpoint manifest was published and read back last.

Background degradation never blocks local work. Use **Back up now** again
after connectivity returns.

## Privacy and retention

R2 objects contain the authored database history, source URLs, research
answers and citations, attachment metadata, and the bytes of referenced
images, PDFs, and other attachments. Kosh v1 does not add client-side
encryption. R2 access control and the private token are therefore the privacy
boundary; use a dedicated private bucket.

Litestream retains exact transaction history for 30 days. Complete checkpoint
manifests and content-addressed media objects are immutable and are not
automatically deleted in v1. Cloudflare lifecycle rules can therefore destroy
otherwise discoverable points and should not be added without a separately
tested retention policy.

Kosh confines every object to
`kosh/v1/backup-sets/<backup-set-id>/`. One installation owns one backup set
at a time. Taking over creates a new replica epoch only after an exact preview;
do it only after the old writer has stopped permanently.

## Routine verification

Use **Find recovery points**, select the newest point, and:

- **Preview restore** to verify the exact transaction plan without writing a
  database;
- **Run recovery drill** to reconstruct and fully validate a disposable copy.

The drill verifies migration heads, both SQLite databases, foreign keys,
attachment bytes, search rebuildability, and citation provenance, then removes
its private workspace. It never changes the live library.

Run a drill after initial setup, after changing the bucket or credentials, and
periodically while the backup matters. A green relational status alone is not
a complete recovery claim.

## Recover onto a clean Mac

Use the exact packaged Kosh application that will open the result. Quit every
Kosh process. The destination must be a brand-new absolute directory; the
command refuses an existing path and never replaces a live library.

For the normal clean-Mac location, first ensure
`$HOME/Library/Application Support` exists and
`$HOME/Library/Application Support/com.rohan.kosh` does not. In a private
Terminal session, export the non-secret target values and read credentials
without echo:

```sh
export KOSH_LITESTREAM_R2_ACCOUNT_ID="your-32-character-account-id"
export KOSH_LITESTREAM_R2_JURISDICTION="DEFAULT"
export KOSH_LITESTREAM_R2_BUCKET="your-private-bucket"
read -r -s KOSH_LITESTREAM_R2_ACCESS_KEY_ID
export KOSH_LITESTREAM_R2_ACCESS_KEY_ID
read -r -s KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY
export KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY

"/Applications/Kosh.app/Contents/MacOS/kosh" \
  recovery remote-restore \
  "your-recorded-backup-set-id" \
  latest \
  "$HOME/Library/Application Support/com.rohan.kosh"
```

Use `EU` or `FEDRAMP` instead of `DEFAULT` only when that is the bucket's
jurisdiction. An exact checkpoint ID may replace `latest`.

The packaged command:

- discovers only complete manifests in the selected backup set;
- restores the manifest's exact Litestream transaction;
- downloads and hashes every retained media object;
- installs only after the staged pair passes full validation;
- reopens through normal Kosh migrations, rebuilds lexical search, and runs a
  full integrity check;
- emits a bounded JSON receipt with counts, never credentials.

After a `PASSED` receipt, clear the shell variables, launch Kosh normally, and
inspect tidbits, revision history, attachments, Exact search results, source
links, and Research citations before declaring recovery complete:

```sh
unset KOSH_LITESTREAM_R2_ACCOUNT_ID
unset KOSH_LITESTREAM_R2_JURISDICTION
unset KOSH_LITESTREAM_R2_BUCKET
unset KOSH_LITESTREAM_R2_ACCESS_KEY_ID
unset KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY
```

If recovery fails, preserve the command's redacted error and the untouched
destination parent. Do not create an empty destination or point the command at
production data to force a retry.

## Disable, revoke, or decommission

Turning **Back up this library** off stops the supervised writer but preserves
the target, Keychain entry, and all R2 recovery data. This is the reversible
choice.

Revoking the bucket token prevents future backup and recovery but does not
delete remote data. Deleting the private bucket destroys the remote history
and media. Removing the Keychain item deletes only the local credential copy;
its account name is the backup set ID. These are separate actions.

For permanent decommissioning:

1. turn backup off and quit Kosh;
2. retain and verify any final recovery record you still need;
3. revoke the bucket-scoped token;
4. delete the dedicated bucket only after its loss is intentional;
5. remove the matching Keychain item.

Kosh does not automatically delete remote recovery objects in v1.

## Developer and release canary

Copy `app/.env.example` to ignored `app/.env`, set mode `0600`, and enter the
bucket-scoped test credentials. The canary always generates a unique backup
set beneath Kosh's fixed prefix and verifies that it is empty after cleanup.

From a clean exact-HEAD checkout with a verified packaged candidate:

```sh
cd app
pnpm release:build:app
KOSH_R2_CANARY_REQUIRE_PACKAGED=1 ../scripts/run-litestream-r2-canary.sh
```

This non-PR lane interrupts and resumes real replication, publishes the
manifest last, drills it, restores through the packaged command into a clean
isolated home, launches the recovered package with hidden windows, rebuilds
search, verifies authored and historical Research citations, and removes the
unique R2 prefix. Its retained reports are redacted; the restored profile and
credentials are never uploaded.
