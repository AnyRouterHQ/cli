anyr for agents and scripts

Read this once, then call anyr without a TTY.

Discover
  anyr commands --json          Every command with its flags, subcommands,
                                aliases and whether it has JSON output
  anyr schema [command [sub]]   Typed map (anyr.schema/v2): flag types, args,
                                auth (none|sk|ak), mutating, exit codes
  anyr <command> --help         The full text for one command

Output
  --json                        JSON on stdout for every command that has a
                                JSON form. On any command, an error becomes one
                                JSON object on stderr:
                                {"error":{"code":"auth","exit":4,"message":"…","hint":"…"}}
  ANYR_OUTPUT=json              The same as passing --json to every command
  --quiet, ANYR_QUIET=1         No update notices and no background update
  NO_COLOR=1                    No ANSI colour (also off whenever stdout is not
                                a terminal)
  Without a TTY, anyr shows no prompts, spinners or update notices. Commands
  that would ask first need --yes.

Exit codes
  0  ok            success
  1  error         the request or command failed
  2  usage         bad flag, argument or subcommand; nothing was sent
  3  not_found     the model, key, id or resource does not exist
  4  auth          no key, a rejected key, or a key without the scope
  5  rate_limited  HTTP 429; retry later

Auth
  ANYROUTER_API_KEY=sk-ar-v1-…  Inference key; no login needed
  ANYROUTER_MANAGEMENT_KEY=ak_… Needed for presets, byok and device removal
  anyr whoami --json            Check which account a key belongs to

Ask a model
  anyr chat "Reply with ok" --json
      {"id":"…","model":"…","content":"ok","finish_reason":"stop","usage":{…}}
  git diff | anyr chat "Review this diff" - --no-stream
  anyr generation <id> --json   Cost, provider and latency of that call
  anyr models --json            The catalog (filter: --free, --type, --search)
  anyr embed "text" --json      Embeddings
  anyr decision --model <id> --state … --questions '{…}' --json

Account
  anyr usage --json             Credits
  anyr logs --json              Recent requests
  anyr keys list --json         API keys
  anyr api <resource> [verb]    Any dashboard resource; anyr api METHOD /path
                                for raw calls (like gh api)

Publish
  anyr artifact publish page.html --json      A page or folder to a URL
  anyr publish <app> doc.json                 A JSON document to an app

Launch a coding agent with an AnyRouter key
  anyr claude --dry-run         Print the command and env, launch nothing
  anyr claude --yes --model anyrouter/auto
