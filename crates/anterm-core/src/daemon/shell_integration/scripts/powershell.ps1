
# --- anterm shell integration (PowerShell) ---
if (-not $env:ANTERM_SHELL_INTEGRATION) {
  $env:ANTERM_SHELL_INTEGRATION = '1'

  $global:__AntermEsc = [char]0x1b
  $global:__AntermBel = [char]0x07
  # Whatever prompt the user's profile settled on; we call through to it.
  $global:__AntermOrigPrompt = $function:prompt
  # Gates the D marker so the first prompt (no command yet) emits no bogus exit.
  $global:__AntermCmdActive = $false

  # Who and where, for the OSC 7 host and the OSC 0 title. Read through .NET
  # rather than the USERNAME / COMPUTERNAME / USERPROFILE environment variables:
  # those spellings exist only on Windows, and on macOS and Linux all three come
  # back empty, which left every pwsh pane there titled `@:` followed by a full
  # un-abbreviated path (issue #583). $HOME is PowerShell's own automatic
  # variable and is right on every platform. Resolved once — none of the three
  # changes within a session.
  $global:__AntermUser = [Environment]::UserName
  $global:__AntermHost = [Environment]::MachineName
  $global:__AntermHome = if ($HOME) { $HOME.Replace('\', '/').TrimEnd('/') } else { '' }

  function global:prompt {
    # $? first: an assignment sets $? to true, so read it before anything else.
    $ok = $?
    $lastExit = $LASTEXITCODE

    if ($global:__AntermCmdActive) {
      $global:__AntermCmdActive = $false
      # $? is the reliable success signal; $LASTEXITCODE can be stale, so only
      # trust it when $? already says the command failed.
      $code = if ($ok) { 0 } elseif ($lastExit) { $lastExit } else { 1 }
      Write-Host -NoNewline "$($global:__AntermEsc)]133;D;$code$($global:__AntermBel)"
    }

    # cwd + title, for real filesystem locations only.
    if ($PWD.Provider.Name -eq 'FileSystem') {
      $fsPath = $PWD.ProviderPath

      # OSC 7 cwd. Escape a literal % as %25 (the daemon percent-decodes the
      # payload) and use forward slashes. Force one leading slash so a Windows
      # drive path (`C:/…`) becomes `/C:/…` — the absolute-path shape the URI
      # expects — while a POSIX path keeps its single slash instead of doubling it.
      $p = $fsPath.Replace('%', '%25').Replace('\', '/')
      if (-not $p.StartsWith('/')) { $p = '/' + $p }
      Write-Host -NoNewline "$($global:__AntermEsc)]7;file://$($global:__AntermHost)$p$($global:__AntermBel)"

      # OSC 0 window/tab title "user@host:dir". Neither a PowerShell profile nor
      # pwsh itself sets a useful title — on macOS pwsh emits an *empty* OSC 0 —
      # so without this every anterm tab running PowerShell stays generic, on every
      # platform. Forward slashes (so anterm's tab-label parser can take the last
      # path segment) and home shown as `~`. Re-emitted each prompt so it tracks
      # cwd; a full-screen app's own title still overrides it while it runs.
      #
      # The home match needs the separator, not just the prefix: with a home of
      # `/Users/ann`, a bare StartsWith also swallows `/Users/annex`, retitling it
      # `~ex`.
      $titlePath = $fsPath.Replace('\', '/')
      if ($global:__AntermHome -and ($titlePath -eq $global:__AntermHome -or
          $titlePath.StartsWith($global:__AntermHome + '/'))) {
        $titlePath = '~' + $titlePath.Substring($global:__AntermHome.Length)
      }
      Write-Host -NoNewline "$($global:__AntermEsc)]0;$($global:__AntermUser)@$($global:__AntermHost):$titlePath$($global:__AntermBel)"
    }

    # Restore the captured status so the user's own prompt sees the real result,
    # then re-restore $LASTEXITCODE afterwards in case the prompt clobbered it.
    $global:LASTEXITCODE = $lastExit
    if (-not $ok) { Write-Error '' -ErrorAction Ignore }
    $base = & $global:__AntermOrigPrompt
    if ($base -is [array]) { $base = $base -join [char]0x0a }
    $global:LASTEXITCODE = $lastExit

    "$($global:__AntermEsc)]133;A$($global:__AntermBel)$base$($global:__AntermEsc)]133;B$($global:__AntermBel)"
  }

  # C (command output begins): wrap PSReadLine's line reader. Force-load the
  # module first so the function exists even if the host hasn't imported it yet;
  # all best-effort (an empty Enter submits no command, so it arms nothing).
  Import-Module PSReadLine -ErrorAction SilentlyContinue
  if ((Test-Path Function:\PSConsoleHostReadLine) -and -not $global:__AntermReadLineWrapped) {
    $global:__AntermReadLineWrapped = $true
    $global:__AntermOrigReadLine = $function:global:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
      $line = & $global:__AntermOrigReadLine
      if (-not [string]::IsNullOrWhiteSpace($line)) {
        $global:__AntermCmdActive = $true
        # Carry the submitted line on the C mark (anterm extension): the daemon
        # detects coding agents from it on Windows, where ConPTY exposes no
        # foreground process group to read an argv from. Truncated (detection
        # only reads the front) and percent-encoded so the payload can't carry
        # a raw `;`, ESC or BEL into the OSC framing. The cut can split a
        # surrogate pair, and a lone high surrogate makes EscapeDataString
        # throw on .NET Framework (PS 5.1) — inside this wrapper that would
        # swallow the submitted line, so drop it and keep the whole mark
        # best-effort: a plain `C` still flips the prompt state.
        $cmd = if ($line.Length -gt 512) { $line.Substring(0, 512) } else { $line }
        if ([char]::IsHighSurrogate($cmd[$cmd.Length - 1])) {
          $cmd = $cmd.Substring(0, $cmd.Length - 1)
        }
        try {
          $cmd = [Uri]::EscapeDataString($cmd)
          Write-Host -NoNewline "$($global:__AntermEsc)]133;C;$cmd$($global:__AntermBel)"
        } catch {
          Write-Host -NoNewline "$($global:__AntermEsc)]133;C$($global:__AntermBel)"
        }
      }
      $line
    }
  }
}
# --- end anterm shell integration ---
