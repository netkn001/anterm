
# --- anterm shell integration (fish) ---
# Guard on *emptiness* (`test -z`), not definedness (`set -q`): `setup()` resets the
# sentinel to an empty-but-exported "" at each spawn boundary, and fish reports an
# empty exported var as *set*, so `not set -q` would skip the install on every fish
# launch (OSC 133 never arms). `-z` matches the zsh/bash guards and the empty reset —
# it installs once for a fresh top-level shell while an inherited `1` still blocks it.
if status is-interactive; and test -z "$ANTERM_SHELL_INTEGRATION"
  set -gx ANTERM_SHELL_INTEGRATION 1

  function __anterm_osc
    printf '\e]%s\a' $argv[1]
  end

  # The daemon percent-decodes the OSC 7 payload; escape literal `%` as %25 so
  # a path like /tmp/a%20b round-trips instead of decoding to /tmp/a b.
  function __anterm_report_cwd
    printf '\e]7;file://%s%s\a' (hostname) (string replace --all '%' '%25' -- $PWD)
  end

  # The C mark carries the submitted line, truncated and with the bytes that
  # would break OSC framing or the daemon's percent-decode escaped (% ESC BEL
  # CR NL) — the Windows agent-detection input (see core::cli_agent). fish
  # command substitution splits output on newlines, so a multi-line command
  # arrives as a list; the final `string join` re-joins it with the escaped
  # newline. `%` must be escaped first (the other escapes introduce `%`).
  function __anterm_preexec --on-event fish_preexec
    set -g __anterm_cmd_active 1
    set -l cmd (string sub -l 512 -- $argv[1] | string replace -a '%' '%25' | string replace -a \e '%1B' | string replace -a \a '%07' | string replace -a \r '%0D' | string join '%0A')
    __anterm_osc "133;C;$cmd"
  end

  # Runs on the fish_prompt *event*, which fires before fish calls the
  # fish_prompt *function* to render the prompt text — i.e. exactly where A
  # (prompt start) belongs.
  function __anterm_precmd --on-event fish_prompt
    set -l ret $status
    if set -q __anterm_cmd_active
      __anterm_osc "133;D;$ret"
      set -e __anterm_cmd_active
    end
    __anterm_report_cwd
    __anterm_osc "133;A"
  end

  functions -c fish_prompt __anterm_original_fish_prompt
  function fish_prompt
    __anterm_original_fish_prompt
    __anterm_osc "133;B"
  end
end
# --- end anterm shell integration ---
