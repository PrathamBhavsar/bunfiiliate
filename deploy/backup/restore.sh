#!/usr/bin/env bash
# Restore a dump (+ optional spool tar) into a target database / spool directory.
#   restore.sh --dump F.dump [--spool-tar F.tar] --target-url URL [--spool-target DIR] [--force]
# Checksums (<file>.sha256) are verified first; a mismatch aborts before anything is touched.
# A non-empty target database or spool dir is refused unless --force (the old spool dir is
# moved aside, never deleted; the DB is restored with --clean --if-exists).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$here/lib.sh"

dump= tar= url= sdir= force=0
while [ $# -gt 0 ]; do
  case $1 in
    --dump) dump=$2; shift 2;;
    --spool-tar) tar=$2; shift 2;;
    --target-url) url=$2; shift 2;;
    --spool-target) sdir=$2; shift 2;;
    --force) force=1; shift;;
    *) die "unknown argument $1";;
  esac
done
[ -n "$dump" ] && [ -n "$url" ] || die "usage: restore.sh --dump F --target-url URL [--spool-tar F --spool-target DIR] [--force]"

verify() { [ -f "$1.sha256" ] || die "missing checksum file $1.sha256"; [ "$(cat "$1.sha256")" = "$(sha_of "$1")" ] || die "checksum mismatch: $1"; }
verify "$dump"
if [ -n "$tar" ]; then verify "$tar"; [ -n "$sdir" ] || die "--spool-target required with --spool-tar"; fi

tables=$(pgx psql "$url" -qAt -c "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
if [ "$tables" != 0 ] && [ "$force" != 1 ]; then die "target database is not empty ($tables tables); pass --force"; fi
if [ -n "$tar" ] && [ -d "$sdir" ] && [ -n "$(ls -A "$sdir" 2>/dev/null)" ] && [ "$force" != 1 ]; then
  die "spool target $sdir is not empty; pass --force"
fi

# --force wipes the public schema first. pg_restore --clean cannot drop the inherited primary keys
# of the clicks partitions (C20), so it would exit non-zero on a partitioned database.
if [ "$force" = 1 ]; then pgx psql "$url" -qv ON_ERROR_STOP=1 -c 'SET client_min_messages = warning; DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public'; fi
pgx pg_restore --no-owner -d "$url" "$dump"

if [ -n "$tar" ]; then
  if [ -d "$sdir" ] && [ -n "$(ls -A "$sdir" 2>/dev/null)" ]; then mv "$sdir" "$sdir.pre-restore.$(date -u +%s)"; fi
  mkdir -p "$sdir"
  command tar -C "$sdir" -xf "$tar"
fi
echo "restore ok"
