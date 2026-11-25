
# --- anterm shell integration (nushell) ---
# Guard on emptiness (`== ""`), not mere definedness, exactly like the other
# shells: setup() resets the sentinel to an empty-but-exported value at each
# spawn boundary, and a nested `nu` inherits the set `1` without re-installing.
if (($env.ANTERM_SHELL_INTEGRATION? | default "") == "") {
  $env.ANTERM_SHELL_INTEGRATION = "1"

  # anterm restores the user's own config.nu at this placeholder — `source` is
  # evaluated at parse time, so the wrapper can only name a file that provably
  # exists; the Rust side resolves the path (the same resolution
  # `$nu.default-config-dir` performs) and substitutes a literal, or a no-op
  # line when there is no config.nu. The order is load-bearing: a config.nu
  # conventionally ends with a wholesale `$env.config = {...}` that would
  # clobber every hook added below, so the hooks must come after it.
  __ANTERM_SOURCE_USER_CONFIG__

  # Hooks live in `$env.config.hooks` as lists. Ensure the skeleton exists
  # before appending to it — a minimal config, or none at all, leaves it
  # missing, and a bare assignment would error instead of creating it.
  if (($env.config.hooks? | default {}) | is-empty) {
    $env.config = ($env.config | upsert hooks { pre_prompt: [], pre_execution: [], env_change: {} })
  }

  $env.config.hooks.pre_prompt = ($env.config.hooks.pre_prompt | default [] | append {||
    # OSC 133 prompt-start (A) and the previous command's exit (D). D is gated
    # on a flag the pre_execution hook arms, so the very first prompt emits no
    # bogus exit status — the same rule every other integration follows.
    let __anterm_osc = {|__anterm_payload| print -n $"(ansi osc)($__anterm_payload)(char bel)" }
    if ($env.__anterm_cmd_active? | default false) {
      hide-env __anterm_cmd_active
      do $__anterm_osc $"133;D;($env.LAST_EXIT_CODE? | default 0)"
    }
    # OSC 7 cwd, so the daemon tracks the pane's directory across `cd`. The
    # payload is percent-decoded on the daemon side, so a literal `%` must be
    # escaped as %25, and a Windows drive path needs the leading slash that
    # makes `C:/…` an absolute URI path (`/C:/…`). Backslashes are separators
    # only on Windows — a Unix path may legally contain one, so that
    # translation is gated on the platform the way nu's own OSC 7 gates it.
    let __anterm_path = if (($nu.os-info.name? | default '') == 'windows') {
      ($env.PWD | str replace -a '\' '/' | str replace -a '%' '%25')
    } else {
      ($env.PWD | str replace -a '%' '%25')
    }
    let __anterm_path = if ($__anterm_path | str starts-with '/') { $__anterm_path } else { '/' + $__anterm_path }
    do $__anterm_osc $"7;file://($env.COMPUTERNAME? | default 'localhost')($__anterm_path)"
    do $__anterm_osc "133;A"
  })

  # OSC 133 command-output-begins (C) plus the flag that gates the D report.
  # Nushell's pre_execution hook receives no command text, so C carries no
  # payload — the daemon still flips to "command running" on the bare mark.
  $env.config.hooks.pre_execution = ($env.config.hooks.pre_execution | default [] | append {||
    $env.__anterm_cmd_active = true
    print -n $"(ansi osc)133;C(char bel)"
  })

  # OSC 133 prompt-end (B) must land after the very last prompt character, and
  # the only hook-shaped place Nushell runs at that point is prompt_indicator —
  # so wrap it, but only when the config defines one. A missing indicator is
  # the built-in default prompt's to render (recent Nushells draw it and their
  # own B mark themselves), and replacing it here would erase the glyph.
  if ((($env.config.prompt_indicator? | default null) | describe) != 'nothing') {
    let __anterm_orig_indicator = $env.config.prompt_indicator
    $env.config.prompt_indicator = {||
      let __anterm_ind = match ($__anterm_orig_indicator | describe) {
        'closure' => (do $__anterm_orig_indicator)
        _ => $__anterm_orig_indicator
      }
      $"($__anterm_ind)(ansi osc)133;B(char bel)"
    }
  }
}
# --- end anterm shell integration ---
