; MiMoBal 自定义安装脚本：安装选项页（桌面快捷方式 / 开机自启动）+ 卸载清理
!include LogicLib.nsh
!include nsDialogs.nsh

; 变量只在安装器侧使用（卸载器编译也会引入本文件，未引用会 warning-as-error）
!ifndef BUILD_UNINSTALLER
Var cbDesktop
Var cbAutoStart
Var optDesktop
Var optAutoStart
Var optVisited
!endif

!ifndef BST_CHECKED
  !define BST_CHECKED 1
!endif
!ifndef BST_UNCHECKED
  !define BST_UNCHECKED 0
!endif

; 静默安装（如自动更新）未经过选项页：保持现状，不改快捷方式与自启动
!macro customInit
  StrCpy $optVisited "0"
  StrCpy $optDesktop "${BST_CHECKED}"
  StrCpy $optAutoStart "${BST_UNCHECKED}"
!macroend

; 目录选择之后、开始安装之前插入选项页（函数体只进安装器，不进卸载器编译）
!ifndef BUILD_UNINSTALLER
!macro customPageAfterChangeDir
  Page custom mimoOptionsCreate mimoOptionsLeave
!macroend

Function mimoOptionsCreate
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 8u 100% 12u "安装选项（之后也可在设置中更改）："
  Pop $0

  ${NSD_CreateCheckbox} 0 30u 100% 12u "创建桌面快捷方式"
  Pop $cbDesktop
  SendMessage $cbDesktop ${BM_SETCHECK} ${BST_CHECKED} 0

  ${NSD_CreateCheckbox} 0 50u 100% 12u "开机自动启动 MiMoBal"
  Pop $cbAutoStart

  nsDialogs::Show
FunctionEnd

Function mimoOptionsLeave
  ${NSD_GetState} $cbDesktop $optDesktop
  ${NSD_GetState} $cbAutoStart $optAutoStart
  StrCpy $optVisited "1"
FunctionEnd
!endif

; 文件与默认快捷方式就位后，按勾选结果调整
!macro customInstall
  ${If} $optVisited == "1"
    ${If} $optDesktop != ${BST_CHECKED}
      Delete "$newDesktopLink"
      System::Call 'Shell32::SHChangeNotify(i 0x8000000, i 0, i 0, i 0)'
    ${EndIf}
    ${If} $optAutoStart == ${BST_CHECKED}
      WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MiMoBal" '"$appExe"'
    ${Else}
      DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MiMoBal"
    ${EndIf}
  ${EndIf}
!macroend

; 卸载时移除自启动注册表项（配置目录按 deleteAppDataOnUninstall=false 保留）
!macro customUnInstall
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MiMoBal"
!macroend
