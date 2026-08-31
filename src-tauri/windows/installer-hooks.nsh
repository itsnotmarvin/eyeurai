; A Tauri updater-launched NSIS installer inherits the old process arguments
; after /ARGS and starts the replacement itself. If EyeUrAI originally came
; from the login item, that includes --hidden, and Windows exits the old app
; before frontend code can sanitize the relaunch.
;
; Write the same short-lived, one-use marker consumed by the new binary. This
; hook lives in the installer artifact, so it also protects the transition from
; v1.3.1 to v1.4.0, whose already-running frontend cannot know about the new
; Rust command.
!macro NSIS_HOOK_POSTINSTALL
  ${If} $UpdateMode = 1
    Push $0

    ClearErrors
    CreateDirectory "$APPDATA\com.eyeurai.desktop"

    ${IfNot} ${Errors}
      Delete "$APPDATA\com.eyeurai.desktop\update-relaunch-visible-v1.tmp"

      ClearErrors
      FileOpen $0 "$APPDATA\com.eyeurai.desktop\update-relaunch-visible-v1.tmp" w

      ${IfNot} ${Errors}
        FileWrite $0 "show-updated-app:${VERSION}"
        FileClose $0

        ${IfNot} ${Errors}
          Delete "$APPDATA\com.eyeurai.desktop\update-relaunch-visible-v1"
          ClearErrors
          ; Rename is the normal atomic handoff. If it fails after the old
          ; marker was deleted, deliberately keep the fully written .tmp file:
          ; the replacement binary validates and consumes either name. That
          ; recovery path prevents an otherwise successful update from
          ; disappearing behind the inherited --hidden argument.
          Rename "$APPDATA\com.eyeurai.desktop\update-relaunch-visible-v1.tmp" "$APPDATA\com.eyeurai.desktop\update-relaunch-visible-v1"
        ${EndIf}
      ${EndIf}
    ${EndIf}

    Pop $0
  ${EndIf}
!macroend
