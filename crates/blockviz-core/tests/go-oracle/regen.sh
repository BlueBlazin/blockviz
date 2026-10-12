#!/bin/sh
# Regenerates go/parser's dumps of the parser fixtures
# (tests/fixtures/go-parser/*.golden) with the Go toolchain on PATH.
# A fixture go/parser rejects gets no golden; tests/go_parser.rs checks it
# by hand instead.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
fixtures="$here/../fixtures/go-parser"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
go build -o "$tmp/dump" "$here/dump.go"
cd "$fixtures"
for f in *.go; do
	golden="${f%.go}.golden"
	"$tmp/dump" "$f" | tail -n +2 >"$tmp/out"
	if [ "$(head -n 1 "$tmp/out")" = ok ]; then
		mv "$tmp/out" "$golden"
	else
		echo "$f: $(cat "$tmp/out") (no golden)"
		rm -f "$golden"
	fi
done
