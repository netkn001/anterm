
# --- anterm shell integration (zsh) ---
if [[ -o interactive ]] && [[ -z "$ANTERM_SHELL_INTEGRATION" ]]; then
  export ANTERM_SHELL_INTEGRATION=1

  # Tell the app where this shell keeps its history. $HISTFILE is set by the
  # rc file just run and is never exported, so the history menu, which reads
  # the file directly, would otherwise only know the default ~/.zsh_history.
  # One line per distinct path, written once.
  if [[ -n "$HISTFILE" && "$HISTFILE" == /* && "$HISTFILE" != */anterm-zdotdir-* && -n "$ANTERM_CONFIG_DIR" ]]; then
    __anterm_hf_list="$ANTERM_CONFIG_DIR/shell-histfiles"
    __anterm_hf_known=
    [[ -r "$__anterm_hf_list" ]] && __anterm_hf_known="$(<"$__anterm_hf_list")"
    if [[ $'\n'"$__anterm_hf_known"$'\n' != *$'\n'"$HISTFILE"$'\n'* ]]; then
      builtin printf '%s\n' "$HISTFILE" >> "$__anterm_hf_list" 2>/dev/null
    fi
    unset __anterm_hf_list __anterm_hf_known
  fi

  # Per-pane history, when the app asked for it. This runs after the user's
  # .zshrc, which is the only reason it can work at all: $HISTFILE is theirs to
  # set, wherever they like, and nothing outside this shell knew where it
  # points until now. Seed once from there so the pane does not start blank,
  # write down how much was seeded — everything past that mark is what this
  # pane adds, and is what goes home when it closes — and only then repoint.
  # zsh loads the history file after .zshrc, so the switch lands before the
  # first line is read.
  if [[ -n "$ANTERM_HISTFILE" ]]; then
    if [[ ! -e "$ANTERM_HISTFILE" ]]; then
      # The origin is recorded even when there is nothing to copy from it yet:
      # a first-ever shell has no history file, and it is exactly that user
      # whose commands would otherwise have nowhere to go when the pane closes.
      # Stage into `.tmp` and rename into place so no reader ever observes a
      # half-seeded file while `tail` is still running.
      __anterm_htmp="$ANTERM_HISTFILE.tmp.$$"
      if [[ -n "$HISTFILE" ]]; then
        builtin printf '%s\n' "$HISTFILE" > "$ANTERM_HISTFILE.origin" 2>/dev/null
        if [[ -r "$HISTFILE" ]]; then
          command tail -n 5000 "$HISTFILE" > "$__anterm_htmp" 2>/dev/null
        fi
      fi
      [[ -e "$__anterm_htmp" ]] || : > "$__anterm_htmp" 2>/dev/null
      command wc -c < "$__anterm_htmp" 2>/dev/null | command tr -d ' \n' \
        > "$ANTERM_HISTFILE.seed" 2>/dev/null
      command tail -c 512 "$__anterm_htmp" > "$ANTERM_HISTFILE.seed.tail" 2>/dev/null
      command mv -f "$__anterm_htmp" "$ANTERM_HISTFILE" 2>/dev/null
      unset __anterm_htmp
    fi
    HISTFILE="$ANTERM_HISTFILE"
    # Write each command to this pane's private file as soon as it runs, not
    # only on clean exit: a forced close, crash, or reboot would otherwise drop
    # every command typed in the pane. Turn off SHARE_HISTORY so this pane
    # stays isolated even if the user's .zshrc enabled sharing.
    setopt INC_APPEND_HISTORY 2>/dev/null
    unsetopt SHARE_HISTORY 2>/dev/null
    # The exit rewrite keeps at most SAVEHIST entries, from a memory capped at
    # HISTSIZE. Smaller than the seed, that rewrite leaves the pane's file
    # shorter than its own seed mark. The caps only govern the pane's private
    # file now, so raising them costs nothing. Left at zero: a user who saves
    # no history has asked for exactly that.
    if (( SAVEHIST > 0 )); then
      (( SAVEHIST < 100000 )) && SAVEHIST=100000
      (( HISTSIZE < SAVEHIST )) && HISTSIZE=$SAVEHIST
    fi
  fi

  __anterm_osc() { builtin printf '\e]%s\a' "$1"; }

  # OSC 7: report the working directory so the app tracks it precisely (used for
  # opening new tabs / splits in the same place). The daemon percent-DECODES the
  # payload (OSC 7 carries a file: URI), so a literal `%` in the path must be
  # escaped as %25 or a dir like `/tmp/a%20b` would decode to `/tmp/a b`.
  __anterm_report_cwd() { builtin printf '\e]7;file://%s%s\a' "${HOST:-localhost}" "${PWD//\%/%25}"; }

  # D (command finished + its exit code) gets its own hook, *prepended* to
  # precmd_functions rather than bundled into __anterm_precmd below: the app only
  # switches back to prompt-editing when D arrives, so every hook that runs
  # before it is a window where keystrokes go raw to the PTY, get kernel-echoed
  # into the grid, and bait zsh's PROMPT_SP into leaving a stray `char + %` line.
  # The user's precmd chain (git-status prompts, conda, …) can take hundreds of
  # ms — D must not wait for it.
  __anterm_precmd_d() {
    local ret=$?
    if [[ -n "$__anterm_cmd_active" ]]; then
      __anterm_osc "133;D;$ret"
      unset __anterm_cmd_active
    fi
  }

  # The rest of the prompt bookkeeping runs right before the prompt is drawn,
  # *after* the user's hooks: report cwd, then open a fresh prompt (A).
  __anterm_precmd() {
    __anterm_report_cwd
    __anterm_osc "133;A"
    # Prompt-end marker (B): emitted at the very end of the prompt — exactly where
    # input begins — by living in PS1 (wrapped in %{...%} so zsh excludes it from
    # prompt width). We (re)append it here in precmd rather than once at load,
    # because prompt frameworks (powerlevel10k / starship / oh-my-zsh) rebuild
    # PS1 in their own precmd and would otherwise drop it. This precmd runs last
    # (added after the user's), and the sentinel check keeps a static PS1 from
    # accumulating duplicate markers.
    [[ "$PS1" != *$'\e]133;B\a'* ]] && PS1="$PS1"$'%{\e]133;B\a%}'
  }

  # preexec runs after the user hits Enter, before the command runs: mark the
  # start of command output (C). We track an "active" flag so the very first
  # prompt (no command yet) doesn't emit a bogus D. The C mark carries the
  # submitted line ($1), truncated (detection only reads the front) and with
  # the bytes that would break OSC framing or the daemon's percent-decode
  # escaped (% ESC BEL CR NL) — the coding-agent detection input on Windows,
  # where ConPTY has no process table to poll (see core::cli_agent).
  __anterm_preexec() {
    __anterm_cmd_active=1
    local cmd=$1
    cmd=${cmd[1,512]}
    cmd=${cmd//\%/%25}
    cmd=${cmd//$'\e'/%1B}
    cmd=${cmd//$'\a'/%07}
    cmd=${cmd//$'\r'/%0D}
    cmd=${cmd//$'\n'/%0A}
    __anterm_osc "133;C;$cmd"
  }

  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __anterm_precmd
  add-zsh-hook preexec __anterm_preexec
  # add-zsh-hook can only append, and the user's hooks are all registered by now
  # (their .zshrc ran before this file) — prepend the D emitter by hand so it's
  # the first thing to run when a command exits. Users who define a classic
  # `precmd()` function still get ahead of us (zsh calls it before the array);
  # that's out of reach without wrapping their function.
  precmd_functions=(__anterm_precmd_d $precmd_functions)

  # Startup kept ZDOTDIR aimed at our throwaway redirector dir so zsh read every
  # one of our startup files. Now they've all run, point it back at the user's
  # real config dir for the live session: tools that resolve state via
  # ${ZDOTDIR:-$HOME} *at runtime* (compinit's .zcompdump, lazily-compiled plugin
  # caches) — and a nested plain `zsh` — must land in the user's dir, not our
  # empty temp one. One-shot: fires on the first precmd, then removes itself.
  __anterm_restore_zdotdir() {
    ZDOTDIR=${ANTERM_USER_ZDOTDIR:-$HOME}
    add-zsh-hook -d precmd __anterm_restore_zdotdir
    unfunction __anterm_restore_zdotdir
  }
  add-zsh-hook precmd __anterm_restore_zdotdir
fi
# --- end anterm shell integration ---
