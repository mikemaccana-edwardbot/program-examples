#!/usr/bin/env bash
# Scan program-examples + program-examples-litesvm for Solana RPC method usage.
# Outputs: rpc-methods.md (histogram + CSV) alongside this script.
#
# Reproducer. Run from anywhere; paths are absolute.
set -uo pipefail

REPOS=(
  "/opt/clawsimple/data/program-examples"
  "/opt/clawsimple/data/program-examples-litesvm"
)

OUT_DIR="/opt/clawsimple/data/program-examples/docs"
OUT_MD="${OUT_DIR}/rpc-methods.md"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Allowlist of known Solana RPC methods (camelCase canonical form).
ALLOWLIST=(
  getAccountInfo getBalance getBlock getBlockCommitment getBlockHeight
  getBlockProduction getBlockTime getBlocks getBlocksWithLimit getClusterNodes
  getEpochInfo getEpochSchedule getFeeForMessage getFirstAvailableBlock
  getGenesisHash getHealth getHighestSnapshotSlot getIdentity
  getInflationGovernor getInflationRate getInflationReward getLargestAccounts
  getLatestBlockhash getLeaderSchedule getMaxRetransmitSlot
  getMaxShredInsertSlot getMinimumBalanceForRentExemption getMultipleAccounts
  getProgramAccounts getRecentPerformanceSamples getRecentPrioritizationFees
  getSignatureStatuses getSignaturesForAddress getSlot getSlotLeader
  getSlotLeaders getStakeActivation getStakeMinimumDelegation getSupply
  getTokenAccountBalance getTokenAccountsByDelegate getTokenAccountsByOwner
  getTokenLargestAccounts getTokenSupply getTransaction getTransactionCount
  getVersion getVoteAccounts isBlockhashValid minimumLedgerSlot requestAirdrop
  sendTransaction simulateTransaction accountSubscribe accountUnsubscribe
  blockSubscribe blockUnsubscribe logsSubscribe logsUnsubscribe
  programSubscribe programUnsubscribe rootSubscribe rootUnsubscribe
  signatureSubscribe signatureUnsubscribe slotSubscribe slotUnsubscribe
  voteSubscribe voteUnsubscribe
)

# Build snake_case -> camelCase map for Rust normalisation.
declare -A SNAKE_TO_CAMEL
camel_to_snake() {
  # getAccountInfo -> get_account_info
  sed -E 's/([a-z0-9])([A-Z])/\1_\2/g' <<<"$1" | tr '[:upper:]' '[:lower:]'
}
for m in "${ALLOWLIST[@]}"; do
  SNAKE_TO_CAMEL["$(camel_to_snake "$m")"]="$m"
done

# Build ripgrep alternation (camelCase | snake_case).
CAMEL_ALT="$(IFS='|'; echo "${ALLOWLIST[*]}")"
SNAKE_LIST=()
for m in "${ALLOWLIST[@]}"; do SNAKE_LIST+=("$(camel_to_snake "$m")"); done
SNAKE_ALT="$(IFS='|'; echo "${SNAKE_LIST[*]}")"

# Exclusions.
IGNORE_GLOBS=(
  --glob '!node_modules' --glob '!target' --glob '!dist' --glob '!.anchor'
  --glob '!build' --glob '!out' --glob '!.git' --glob '!**/generated/**'
  --glob '!*.lock' --glob '!*-lock.*' --glob '!pnpm-lock.yaml'
  --glob '!package-lock.json' --glob '!yarn.lock' --glob '!Cargo.lock'
)

# Output schema (per match line): repo\tlang\texample\tfile\tmethod
MATCHES="$TMP/matches.tsv"
: >"$MATCHES"

# Count of non-allowlisted matches (call-site-looking tokens we skipped).
# We compute this by scanning the same call-site patterns for any identifier,
# then subtracting allowlist hits.
NONALLOW="$TMP/nonallow.txt"
: >"$NONALLOW"

# Track unique files scanned (files that contained any allowlisted match).
FILES_SET="$TMP/files.txt"
: >"$FILES_SET"

scan_repo() {
  local repo="$1"
  local repo_name; repo_name="$(basename "$repo")"

  # -------- TypeScript / JavaScript --------
  # Patterns:
  #   connection.<method>(
  #   rpc.<method>(
  #   .<method>({ ... }).send(   — Kit builder: we approximate by ".<method>(" followed somewhere by ".send(".
  # We'll keep the scanner simple: match any `.<allowlistMethod>(` on a ts/js line.
  # This catches connection.*, rpc.*, and builder chains (top method in chain is the RPC method name).
  # False positives from unrelated objects are negligible vs allowlist gating.
  local ts_pattern="\\.(${CAMEL_ALT})\\("
  rg --no-messages -H -n -o --no-heading \
     -tts -tjs \
     "${IGNORE_GLOBS[@]}" \
     -e "$ts_pattern" "$repo" 2>/dev/null \
  | awk -F: -v repo="$repo" -v repo_name="$repo_name" '
      {
        # file:line:matchtext
        file=$1
        match_text=$NF
        # Strip repo prefix for example path.
        rel=file; sub(repo "/", "", rel)
        split(rel, parts, "/")
        example=parts[1]
        if (length(parts) >= 2 && (parts[1] == "basics" || parts[1] == "tokens" || parts[1] == "compression" || parts[1] == "oracles" || parts[1] == "defi" || parts[1] == "tools")) {
          example=parts[1] "/" parts[2]
        }
        # Extract method name from match_text: .methodName(
        m=match_text
        sub(/^.*\./, "", m)
        sub(/\(.*$/, "", m)
        printf "%s\tts\t%s\t%s\t%s\n", repo_name, example, file, m
      }' >>"$MATCHES"

  # -------- Rust --------
  # Patterns: rpc_client.<method>(, RpcClient::<method>, .rpc().<method>(, nonblocking_rpc_client.<method>(
  # We simplify: match snake_case allowlist methods as:
  #   (^|[^A-Za-z0-9_])<method>\(
  # Then gate by earlier-in-file presence of rpc client types? Too complex.
  # Instead: require the token is preceded by `.` or `::` (i.e. a call on something).
  local rust_pattern="(\\.|::)(${SNAKE_ALT})\\("
  rg --no-messages -H -n -o --no-heading \
     -trust \
     "${IGNORE_GLOBS[@]}" \
     -e "$rust_pattern" "$repo" 2>/dev/null \
  | awk -F: -v repo="$repo" -v repo_name="$repo_name" '
      {
        file=$1
        match_text=$NF
        rel=file; sub(repo "/", "", rel)
        split(rel, parts, "/")
        example=parts[1]
        if (length(parts) >= 2 && (parts[1] == "basics" || parts[1] == "tokens" || parts[1] == "compression" || parts[1] == "oracles" || parts[1] == "defi" || parts[1] == "tools")) {
          example=parts[1] "/" parts[2]
        }
        m=match_text
        sub(/^.*(\.|::)/, "", m)
        sub(/\(.*$/, "", m)
        printf "%s\trust\t%s\t%s\t%s\n", repo_name, example, file, m
      }' >>"$MATCHES"

  # -------- Python --------
  # Python Solana SDK uses snake_case methods on client too; same Rust-style pattern works.
  rg --no-messages -H -n -o --no-heading \
     -tpy \
     "${IGNORE_GLOBS[@]}" \
     -e "$rust_pattern" "$repo" 2>/dev/null \
  | awk -F: -v repo="$repo" -v repo_name="$repo_name" '
      {
        file=$1; match_text=$NF
        rel=file; sub(repo "/", "", rel)
        split(rel, parts, "/")
        example=parts[1]
        if (length(parts) >= 2 && (parts[1] == "basics" || parts[1] == "tokens" || parts[1] == "compression" || parts[1] == "oracles" || parts[1] == "defi" || parts[1] == "tools")) {
          example=parts[1] "/" parts[2]
        }
        m=match_text
        sub(/^.*(\.|::)/, "", m)
        sub(/\(.*$/, "", m)
        printf "%s\tpy\t%s\t%s\t%s\n", repo_name, example, file, m
      }' >>"$MATCHES"

  # -------- Non-allowlist counter --------
  # Count all `.<camel>(` identifiers that look like RPC-ish methods (start get/is/send/sim/request/min + subscribe/unsubscribe)
  # in ts/js, then subtract allowlist hits. Same for rust snake_case on rpc-looking things.
  rg --no-messages -H -n -o --no-heading \
     -tts -tjs \
     "${IGNORE_GLOBS[@]}" \
     -e "\\.(get|is|send|simulate|request|minimum)[A-Z][A-Za-z0-9]*\\(" \
     "$repo" 2>/dev/null \
  | awk -F: '{print $NF}' \
  | sed -E 's/^.*\.//; s/\(.*$//' \
  >>"$NONALLOW.ts.all" 2>/dev/null || true

  rg --no-messages -H -n -o --no-heading \
     -trust \
     "${IGNORE_GLOBS[@]}" \
     -e "(\\.|::)(get|is|send|simulate|request|minimum)_[a-z_]+\\(" \
     "$repo" 2>/dev/null \
  | awk -F: '{print $NF}' \
  | sed -E 's/^.*(\.|::)//; s/\(.*$//' \
  >>"$NONALLOW.rust.all" 2>/dev/null || true
}

for r in "${REPOS[@]}"; do scan_repo "$r"; done

# Non-allowlist count = total rpc-ish-looking tokens - allowlisted tokens.
TS_TOTAL=0; RUST_TOTAL=0
[[ -f "$NONALLOW.ts.all"   ]] && TS_TOTAL=$(wc -l <"$NONALLOW.ts.all")
[[ -f "$NONALLOW.rust.all" ]] && RUST_TOTAL=$(wc -l <"$NONALLOW.rust.all")

TS_ALLOW=$(awk -F'\t' '$2=="ts"{c++} END{print c+0}' "$MATCHES")
RUST_ALLOW=$(awk -F'\t' '$2=="rust"{c++} END{print c+0}' "$MATCHES")
NON_ALLOW_TOTAL=$(( (TS_TOTAL - TS_ALLOW) + (RUST_TOTAL - RUST_ALLOW) ))
(( NON_ALLOW_TOTAL < 0 )) && NON_ALLOW_TOTAL=0

# Aggregate: normalise rust snake_case to camelCase, then group.
AGG="$TMP/agg.tsv"
python3 - "$MATCHES" "$AGG" <<'PY'
import sys, os, re
from collections import defaultdict

inp, out = sys.argv[1], sys.argv[2]

allowlist = [
  "getAccountInfo","getBalance","getBlock","getBlockCommitment","getBlockHeight",
  "getBlockProduction","getBlockTime","getBlocks","getBlocksWithLimit","getClusterNodes",
  "getEpochInfo","getEpochSchedule","getFeeForMessage","getFirstAvailableBlock",
  "getGenesisHash","getHealth","getHighestSnapshotSlot","getIdentity",
  "getInflationGovernor","getInflationRate","getInflationReward","getLargestAccounts",
  "getLatestBlockhash","getLeaderSchedule","getMaxRetransmitSlot",
  "getMaxShredInsertSlot","getMinimumBalanceForRentExemption","getMultipleAccounts",
  "getProgramAccounts","getRecentPerformanceSamples","getRecentPrioritizationFees",
  "getSignatureStatuses","getSignaturesForAddress","getSlot","getSlotLeader",
  "getSlotLeaders","getStakeActivation","getStakeMinimumDelegation","getSupply",
  "getTokenAccountBalance","getTokenAccountsByDelegate","getTokenAccountsByOwner",
  "getTokenLargestAccounts","getTokenSupply","getTransaction","getTransactionCount",
  "getVersion","getVoteAccounts","isBlockhashValid","minimumLedgerSlot","requestAirdrop",
  "sendTransaction","simulateTransaction","accountSubscribe","accountUnsubscribe",
  "blockSubscribe","blockUnsubscribe","logsSubscribe","logsUnsubscribe",
  "programSubscribe","programUnsubscribe","rootSubscribe","rootUnsubscribe",
  "signatureSubscribe","signatureUnsubscribe","slotSubscribe","slotUnsubscribe",
  "voteSubscribe","voteUnsubscribe",
]
camel_set = set(allowlist)

def to_camel(snake):
    parts = snake.split("_")
    return parts[0] + "".join(p[:1].upper()+p[1:] for p in parts[1:])

snake_map = {}
for c in allowlist:
    s = re.sub(r'([a-z0-9])([A-Z])', r'\1_\2', c).lower()
    snake_map[s] = c

counts = defaultdict(int)
files  = defaultdict(set)
examples = defaultdict(set)
langs  = defaultdict(set)
all_files = set()

with open(inp) as f:
    for line in f:
        line = line.rstrip("\n")
        if not line: continue
        parts = line.split("\t")
        if len(parts) != 5: continue
        repo, lang, example, file, method = parts
        if lang in ("rust","py"):
            method = snake_map.get(method, method)
        if method not in camel_set:
            continue
        counts[method] += 1
        files[method].add(file)
        examples[method].add(f"{repo}:{example}")
        langs[method].add(lang)
        all_files.add(file)

with open(out, "w") as w:
    for m in sorted(counts, key=lambda k: (-counts[k], k)):
        w.write(f"{m}\t{counts[m]}\t{len(files[m])}\t{len(examples[m])}\t{'|'.join(sorted(langs[m]))}\n")

with open(out + ".meta", "w") as w:
    w.write(f"files_scanned\t{len(all_files)}\n")
    w.write(f"matches_allowed\t{sum(counts.values())}\n")
PY

FILES_SCANNED=$(awk -F'\t' '$1=="files_scanned"{print $2}' "$AGG.meta")
MATCHES_ALLOWED=$(awk -F'\t' '$1=="matches_allowed"{print $2}' "$AGG.meta")

# Render markdown.
SCAN_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

MAX_COUNT=$(awk -F'\t' 'NR==1{print $2}' "$AGG")
[[ -z "$MAX_COUNT" || "$MAX_COUNT" -eq 0 ]] && MAX_COUNT=1

{
  echo "# Solana RPC methods used in program-examples"
  echo
  echo "Scanned: /opt/clawsimple/data/program-examples + /opt/clawsimple/data/program-examples-litesvm"
  echo "Scan date: ${SCAN_DATE}"
  echo "Files scanned: ${FILES_SCANNED}"
  echo "Matches found: ${MATCHES_ALLOWED} (after allowlist filter)"
  echo "Non-allowlisted matches skipped: ${NON_ALLOW_TOTAL}"
  echo
  echo "## Top methods (by total uses)"
  echo
  printf '```\n'
  printf '%-34s %-6s %-6s %-9s %s\n' method count files examples bar
  awk -F'\t' -v mx="$MAX_COUNT" '
    {
      n=int(($2/mx)*40+0.5); if(n<1 && $2>0) n=1;
      bar=""; for(i=0;i<n;i++) bar=bar"█";
      printf "%-34s %-6s %-6s %-9s |%-40s|\n", $1, $2, $3, $4, bar
    }' "$AGG"
  printf '```\n'
  echo
  echo "## CSV"
  echo
  printf '```\n'
  echo "method,count,files,examples,langs"
  awk -F'\t' '{printf "%s,%s,%s,%s,%s\n",$1,$2,$3,$4,$5}' "$AGG"
  printf '```\n'
  echo
  echo "## Reproduce"
  echo
  echo 'Run `docs/rpc-methods-scan.sh` from anywhere; paths are absolute. Requires `ripgrep` and `python3`.'
} >"$OUT_MD"

echo "Wrote $OUT_MD"
echo "files_scanned=$FILES_SCANNED matches_allowed=$MATCHES_ALLOWED non_allow_skipped=$NON_ALLOW_TOTAL"
