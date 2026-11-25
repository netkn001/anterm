
# --- anterm shell integration (bash) ---
if [[ $- == *i* ]] && [[ -z "$ANTERM_SHELL_INTEGRATION" ]]; then
  export ANTERM_SHELL_INTEGRATION=1

  # Tell the app where this shell keeps its history. $HISTFILE is set by the
  # rc file just run and is never exported, so the history menu, which reads
  # the file directly, would otherwise only know the default ~/.bash_history.
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

  # Per-pane history — see the zsh block for why this can only be done here,
  # after the user's rc has decided where their history lives. bash loads the
  # file after the startup files too, so repointing here still precedes the
  # load.
  if [[ -n "$ANTERM_HISTFILE" ]]; then
    if [[ ! -e "$ANTERM_HISTFILE" ]]; then
      # The origin is recorded even when there is nothing to copy from it yet:
      # a first-ever shell has no history file, and it is exactly that user
      # whose commands would otherwise have nowhere to go when the pane closes.
      # Stage into `.tmp` and rename into place so no reader sees a half-seeded
      # file while `tail` is still running.
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
    shopt -s histappend 2>/dev/null
    __anterm_per_pane_hist=1
    # On exit bash rewrites the file from a memory capped at HISTSIZE, then
    # truncates it to HISTFILESIZE lines — both default to 500, which is
    # smaller than the seed. Both caps only govern the pane's private file now,
    # so raising them costs nothing. Negative means unlimited and is already
    # enough.
    if (( ${HISTSIZE:-500} >= 0 && ${HISTSIZE:-500} < 100000 )) 2>/dev/null; then
      HISTSIZE=100000
    fi
    if (( ${HISTFILESIZE:-500} >= 0 && ${HISTFILESIZE:-500} < 100000 )) 2>/dev/null; then
      HISTFILESIZE=100000
    fi
  fi

  __anterm_osc() { builtin printf '\e]%s\a' "$1"; }
  # Escape literal `%` as %25 — the daemon percent-decodes the OSC 7 payload.
  #
  # Under Git Bash (msys) `$PWD` is an msys path — `/c/Users/x`, and `/tmp` for
  # mounts with no drive at all. The daemon runs Windows-side, where `/c/Users/x`
  # is not absolute but *drive-relative*, so it resolves to a bogus `C:\c\Users\x`
  # (see `pane::strip_uri_drive_slash`, which only un-prefixes `/C:/…`). `pwd -W`
  # is msys's own translation to the real Windows path, and it resolves mounts
  # that have a real backing directory (`/tmp` -> `C:/Users/x/AppData/Local/Temp`).
  # It has no leading slash, so add one to make it the absolute-path shape a
  # file: URI expects.
  #
  # For msys-only virtual mounts (`/proc`, `/dev`) there *is* no Windows path
  # and `pwd -W` is the identity, so require a drive letter and stay silent
  # otherwise — a `/proc` payload would land as drive-relative `C:\proc` and
  # fail the next spawn, whereas reporting nothing leaves the daemon holding
  # the last usable cwd. Testing the shape beats testing for a leading slash,
  # which cannot tell a translated path from an untranslated one.
  #
  # The branch is resolved once at install time; the `$(…)` inside still forks
  # per prompt (~6.8 ms under msys, vs ~0.3 ms for the plain `$PWD` path), which
  # is the price of a correct path and only paid by Git Bash panes.
  if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]]; then
    __anterm_report_cwd() {
      local d
      d="$(builtin pwd -W 2>/dev/null)" || return 0
      [[ "$d" == ?:* ]] || return 0
      builtin printf '\e]7;file://%s/%s\a' "${HOSTNAME:-localhost}" "${d//\%/%25}"
    }
  else
    __anterm_report_cwd() { builtin printf '\e]7;file://%s%s\a' "${HOSTNAME:-localhost}" "${PWD//\%/%25}"; }
  fi

  # Own hook for D, prepended to precmd_functions (same rationale as the zsh
  # path): the app flips back to prompt-editing on D, so it must fire the
  # instant the command exits, not after the user's precmd functions.
  __anterm_precmd_d() {
    local ret=$?
    if [[ -n "$__anterm_cmd_active" ]]; then
      __anterm_osc "133;D;$ret"
      unset __anterm_cmd_active
    fi
    return $ret
  }

  __anterm_precmd() {
    local ret=$?
    if [[ -n "${__anterm_per_pane_hist:-}" ]]; then
      builtin history -a 2>/dev/null
    fi
    __anterm_report_cwd
    __anterm_osc "133;A"
    # Prompt-end marker (B), wrapped in \[...\] so readline excludes it from the
    # prompt's on-screen width. Re-appended every precmd (like the zsh path)
    # since prompt frameworks that rebuild PS1 in their own precmd would
    # otherwise drop it; the case-check keeps a static PS1 from accumulating
    # duplicates.
    case "$PS1" in
      *'\[\033]133;B\a\]'*) ;;
      *) PS1="$PS1"'\[\033]133;B\a\]' ;;
    esac
    return $ret
  }

  # The C mark carries the submitted line ($1, from bash-preexec), truncated
  # and escaped the same way as the zsh path — the Windows agent-detection
  # input (git-bash; see core::cli_agent).
  __anterm_preexec() {
    __anterm_cmd_active=1
    local cmd=${1:0:512}
    cmd=${cmd//\%/%25}
    cmd=${cmd//$'\e'/%1B}
    cmd=${cmd//$'\a'/%07}
    cmd=${cmd//$'\r'/%0D}
    cmd=${cmd//$'\n'/%0A}
    __anterm_osc "133;C;$cmd"
  }

  if [[ -z "${bash_preexec_imported:-}" ]]; then
    # --- vendored from bash-preexec.sh (https://github.com/rcaloras/bash-preexec, MIT) ---
    bash_preexec_imported="defined"
    __bp_imported="$bash_preexec_imported"

    __bp_last_ret_value="$?"
    BP_PIPESTATUS=("${PIPESTATUS[@]}")
    __bp_last_argument_prev_command="$_"
    __bp_inside_precmd=0
    __bp_inside_preexec=0
    __bp_preexec_interactive_mode=""
    __bp_install_string=$'__bp_trap_string="$(trap -p DEBUG)"\ntrap - DEBUG\n__bp_install'

    declare -a precmd_functions
    declare -a preexec_functions

    __bp_require_not_readonly() {
      local var
      for var; do
        if ! ( unset "$var" 2> /dev/null ); then
          echo "bash-preexec requires write access to ${var}" >&2
          return 1
        fi
      done
    }

    __bp_trim_whitespace() {
      local var=${1:?} text=${2:-}
      text="${text#"${text%%[![:space:]]*}"}"
      text="${text%"${text##*[![:space:]]}"}"
      printf -v "$var" '%s' "$text"
    }

    __bp_sanitize_string() {
      local var=${1:?} text=${2:-} sanitized
      __bp_trim_whitespace sanitized "$text"
      sanitized=${sanitized%;}
      sanitized=${sanitized#;}
      __bp_trim_whitespace sanitized "$sanitized"
      printf -v "$var" '%s' "$sanitized"
    }

    __bp_interactive_mode() { __bp_preexec_interactive_mode="on"; }

    __bp_precmd_invoke_cmd() {
      __bp_last_ret_value="$?" BP_PIPESTATUS=("${PIPESTATUS[@]}")
      if (( __bp_inside_precmd > 0 )); then return; fi
      local __bp_inside_precmd=1
      local precmd_function
      for precmd_function in "${precmd_functions[@]}"; do
        if type -t "$precmd_function" 1>/dev/null; then
          __bp_set_ret_value "$__bp_last_ret_value" "$__bp_last_argument_prev_command"
          "$precmd_function"
        fi
      done
      __bp_set_ret_value "$__bp_last_ret_value"
    }

    __bp_set_ret_value() { return ${1:+"$1"}; }

    __bp_in_prompt_command() {
      local prompt_command_array IFS=$'\n;'
      read -rd '' -a prompt_command_array <<< "${PROMPT_COMMAND[*]:-}"
      local trimmed_arg
      __bp_trim_whitespace trimmed_arg "${1:-}"
      local command trimmed_command
      for command in "${prompt_command_array[@]:-}"; do
        __bp_trim_whitespace trimmed_command "$command"
        if [[ "$trimmed_command" == "$trimmed_arg" ]]; then return 0; fi
      done
      return 1
    }

    __bp_preexec_invoke_exec() {
      __bp_last_argument_prev_command="${1:-}"
      if (( __bp_inside_preexec > 0 )); then return; fi
      local __bp_inside_preexec=1
      if [[ ! -t 1 && -z "${__bp_delay_install:-}" ]]; then return; fi
      if [[ -n "${COMP_LINE:-}" ]]; then return; fi
      if [[ -n "${READLINE_LINE+x}" ]]; then return; fi
      if [[ -z "${__bp_preexec_interactive_mode:-}" ]]; then
        return
      else
        if [[ 0 -eq "${BASH_SUBSHELL:-}" ]]; then
          __bp_preexec_interactive_mode=""
        fi
      fi
      if __bp_in_prompt_command "${BASH_COMMAND:-}"; then
        __bp_preexec_interactive_mode=""
        return
      fi
      local this_command
      this_command=$(
        export LC_ALL=C
        HISTTIMEFORMAT='' builtin history 1 | sed '1 s/^ *[0-9][0-9]*[* ] //'
      )
      if [[ -z "$this_command" ]]; then return; fi
      local preexec_function
      local preexec_function_ret_value
      local preexec_ret_value=0
      for preexec_function in "${preexec_functions[@]:-}"; do
        if type -t "$preexec_function" 1>/dev/null; then
          __bp_set_ret_value "${__bp_last_ret_value:-}"
          "$preexec_function" "$this_command"
          preexec_function_ret_value="$?"
          if [[ "$preexec_function_ret_value" != 0 ]]; then
            preexec_ret_value="$preexec_function_ret_value"
          fi
        fi
      done
      __bp_set_ret_value "$preexec_ret_value" "$__bp_last_argument_prev_command"
    }

    __bp_install() {
      if [[ "${PROMPT_COMMAND[*]:-}" == *"__bp_precmd_invoke_cmd"* ]]; then return 1; fi
      trap '__bp_preexec_invoke_exec "$_"' DEBUG
      local prior_trap
      prior_trap=$(sed "s/[^']*'\(.*\)'[^']*/\1/" <<<"${__bp_trap_string:-}")
      unset __bp_trap_string
      if [[ -n "$prior_trap" ]]; then
        eval '__bp_original_debug_trap() {
          '"$prior_trap"'
        }'
        preexec_functions+=(__bp_original_debug_trap)
      fi
      if [[ -n "${__bp_enable_subshells:-}" ]]; then
        set -o functrace > /dev/null 2>&1
        shopt -s extdebug > /dev/null 2>&1
      fi;
      local existing_prompt_command
      existing_prompt_command="${PROMPT_COMMAND:-}"
      existing_prompt_command="${existing_prompt_command//$__bp_install_string/:}"
      existing_prompt_command="${existing_prompt_command//$'\n':$'\n'/$'\n'}"
      existing_prompt_command="${existing_prompt_command//$'\n':;/$'\n'}"
      __bp_sanitize_string existing_prompt_command "$existing_prompt_command"
      if [[ "${existing_prompt_command:-:}" == ":" ]]; then
        existing_prompt_command=
      fi
      PROMPT_COMMAND='__bp_precmd_invoke_cmd'
      PROMPT_COMMAND+=${existing_prompt_command:+$'\n'$existing_prompt_command}
      if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
        PROMPT_COMMAND+=('__bp_interactive_mode')
      else
        PROMPT_COMMAND+=$'\n__bp_interactive_mode'
      fi
      precmd_functions+=(precmd)
      preexec_functions+=(preexec)
      __bp_precmd_invoke_cmd
      __bp_interactive_mode
    }

    __bp_install_after_session_init() {
      __bp_require_not_readonly PROMPT_COMMAND HISTCONTROL HISTTIMEFORMAT || return
      local sanitized_prompt_command
      __bp_sanitize_string sanitized_prompt_command "${PROMPT_COMMAND:-}"
      if [[ -n "$sanitized_prompt_command" ]]; then
        PROMPT_COMMAND=${sanitized_prompt_command}$'\n'
      fi;
      PROMPT_COMMAND+=${__bp_install_string}
    }
    # --- end vendored bash-preexec.sh ---

    __bp_install_after_session_init
  fi

  # D first (before any user precmds bash-preexec already knows about), the
  # prompt bookkeeping last — mirroring the zsh registration order.
  precmd_functions=(__anterm_precmd_d "${precmd_functions[@]}")
  precmd_functions+=(__anterm_precmd)
  preexec_functions+=(__anterm_preexec)
fi
# --- end anterm shell integration ---
