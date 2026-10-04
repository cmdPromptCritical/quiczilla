#!/usr/bin/env bash
# S5 Native directory mode.
# Sends a tree with nesting, empty directories, Unicode, spaces, a 254-byte
# name, many small files, a larger binary, and a source symlink. Asserts the
# received file and directory manifests match, the symlink was skipped, and
# the receipt counts agree with the receiver acknowledgement.
source "$(dirname -- "${BASH_SOURCE[0]}")/../lib/common.sh"

prepare_dirs
trap cleanup_remote_dir EXIT
root="$WORK/tree"

mkdir -p "$root/a/b/c/d/e" "$root/empty-leaf" "$root/nested-empty/inner" "$root/with space"
printf 'hello\n' > "$root/a/b/c/d/e/deep.txt"
printf 'unicode\n' > "$root/résumé.txt"
printf 'cjk\n' > "$root/日本語.txt"
printf 'emoji\n' > "$root/emoji_🦖.txt"
printf 'space\n' > "$root/with space/file name.txt"
long_name="$(printf 'L%.0s' $(seq 1 250)).txt"
printf 'long\n' > "$root/$long_name"
mkdir -p "$root/many"
for i in $(seq 1 300); do
  head -c $((RANDOM % 4096)) /dev/urandom > "$root/many/f$i.bin"
done
make_file "$root/a/blob.dat" 5242880
ln -s "a/blob.dat" "$root/link-to-blob"

qz dir "$root" "$TARGET:$REMOTE_DIR/" --no-progress --json --checksum "${QZ_SSH_ARGS[@]}"
expect_ok dir
expect_transport dir 'direct-quic|stun-quic' >/dev/null || exit 1
[[ "$(receipt dir '.kind')" == "directory" ]] || fail "receipt kind is not directory"

# Expected tree excludes the source symlink.
expected="$WORK/expected"
cp -a "$root" "$expected"
rm -f "$expected/link-to-blob"
assert_same_tree "$expected" "$REMOTE_DIR/tree"
remote_exists "$REMOTE_DIR/tree/link-to-blob" && fail "source symlink was transferred"
if remote "find $(q "$REMOTE_DIR") -name '*.quic-part' | grep -q ."; then
  fail "staging files remain after directory transfer"
fi

files="$(receipt dir '.files')"
receiver_files="$(receipt dir '.receiver_files')"
expected_files="$(find "$expected" -type f | wc -l)"
[[ "$files" == "$receiver_files" ]] || fail "sender files ($files) != receiver files ($receiver_files)"
[[ "$files" == "$expected_files" ]] || fail "receipt files ($files) != expected ($expected_files)"
metric files "$files"
metric packs "$(receipt dir '.packs')"
metric seconds "$QZ_ELAPSED"
