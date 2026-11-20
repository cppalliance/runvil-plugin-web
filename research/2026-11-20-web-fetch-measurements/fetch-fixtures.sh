#!/bin/sh
# Downloads the fixtures into ./fixtures and ./pdfs. Pages change over time,
# so the listing pages (Hacker News, lobste.rs, the forum, DuckDuckGo) need
# new phrases in html/criteria.json after a fresh download.
set -eu
cd "$(dirname "$0")"
mkdir -p fixtures pdfs
ua='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36'
python3 -c 'import json; [print(k, v["url"]) for k, v in json.load(open("html/criteria.json")).items()]' |
while read -r name url; do
  [ "$name" = issue23-profile ] && continue   # committed: the page from the issue
  curl -sSL -A "$ua" -o "fixtures/$name.html" "$url"
done
curl -sSL -A "$ua" -o pdfs/paper-1col-attention.pdf https://arxiv.org/pdf/1706.03762
curl -sSL -A "$ua" -o pdfs/paper-2col-bert.pdf https://arxiv.org/pdf/1810.04805
curl -sSL -A "$ua" -o pdfs/tables-h8.pdf https://www.federalreserve.gov/releases/h8/current/h8.pdf
curl -sSL -A "$ua" -o pdfs/tables-h41.pdf https://www.federalreserve.gov/releases/h41/current/h41.pdf
