# mina-json-bytes

Carries bytes that are not valid UTF-8 through `jq`.

Mina allows arbitrary bytes in some text fields, so its JSON is not always valid
UTF-8. Many tools process this JSON with `jq`, and each such step can change the
content: `jq` replaces each invalid byte with U+FFFD, but these bytes must stay
exact.

Use this tool for each `jq` command that writes such JSON. A read-only `jq -r` on
a field that is always valid UTF-8 does not need it.

## Usage

```bash
# Filter: JSON comes from stdin.
mina-json-bytes jq 'del(.genesis)' < config.json > out.json

# JSON from files: --slurpfile inputs are encoded too.
mina-json-bytes jq -n --slurpfile genesis genesis.json --slurpfile next next.json \
    '{ledger: {accounts: $genesis[0]}, next: {accounts: $next[0]}}' > config.json

# The two steps, for a tool that is not jq.
mina-json-bytes encode < ledger.json | some-text-tool | mina-json-bytes decode > out.json
```

`encode` replaces each byte that is not valid UTF-8 with the ASCII token
`__MINA_BYTE_xx__` (`xx`: the byte in lower-case hex). Where the input already
holds the text `__MINA_BYTE_`, `encode` also writes the first `_` of it as the
token `__MINA_BYTE_5f__`, so that `decode` cannot take that text for a token.
`decode` replaces each token with its byte. All other bytes stay the same, so
`decode(encode(x)) == x` for every input.

## Rules of the `jq` subcommand

jq opens its file arguments itself, so a file given to jq directly does not go
through `encode`. For this reason:

- JSON comes from stdin or from `--slurpfile`.
- An argument after the filter is an error, because jq would read it as a file.
- `--rawfile`, `-f`, `--from-file`, `--args` and `--jsonargs` are errors.
  `src/jq.rs` has the list of rules.
- With `-n`, the tool does not read stdin.

Exit codes: the exit code of jq, `1` for a failure, `2` for a usage error.

The output goes to stdout while jq runs, so a failure can leave a partial
output. Write to a temporary file, and move it only after a success:

```bash
set -o pipefail
mina-json-bytes jq 'del(.genesis)' < config.json > out.tmp && mv out.tmp out.json
```

## Limits

- A jq filter sees the token and not the byte. For example, `length` or `test`
  on such a field gives a result for the token.
- jq still changes the format of its output: whitespace, escapes such as
  `é`, and the precision of numbers above 2^53. For a copy that must be
  identical except for one removed part, do not use jq.
- `encode` and `decode` stream, and use about 4 MB of memory. jq loads all of
  the input: a 734 MB ledger took 6.4 GB of memory in `jq -c`.
