@echo off
chcp 65001 >nul
cd /d "%~dp0"
set "GH=git@github.com:YOUR_USER/findany.git"
set "GITEE=git@gitee.com:YOUR_USER/findany.git"
echo.
echo === findany 发布: SSH 推送到 GitHub + Gitee ===
echo.
echo 1. 先把本文件顶部 GH / GITEE 的 YOUR_USER 改成你的用户名
echo 2. 在 GitHub / Gitee 建好同名空仓库 findany
echo 3. 确保本机已有 SSH 密钥并添加到两端 (见 README 发布节)
if not exist "%USERPROFILE%\.ssh" mkdir "%USERPROFILE%\.ssh" >nul 2>nul
if not exist "%USERPROFILE%\.ssh\id_ed25519" (
  echo 未检测到 SSH 密钥, 正在自动生成...
  ssh-keygen -t ed25519 -C "you@example.com" -f "%USERPROFILE%\.ssh\id_ed25519" -N "" -q
)
echo.
echo 你的 SSH 公钥 (请复制并添加到 GitHub 和 Gitee 的 SSH Keys):
type "%USERPROFILE%\.ssh\id_ed25519.pub"
echo.
if not exist .git git init
git branch -M main
git add -A
git commit -m "init: findany" 1>nul 2>nul
echo 强制重置远端为 SSH 地址 (覆盖旧 HTTPS 远端)...
git remote remove origin >nul 2>nul
git remote remove gitee >nul 2>nul
git remote add origin %GH%
git remote add gitee %GITEE%
git remote set-url origin %GH%
git remote set-url gitee %GITEE%
echo.
echo 推送 GitHub...
git push -u origin main
if errorlevel 1 echo   失败: 请确认 %GH% 正确、仓库已建、公钥已添加到 GitHub。
echo 推送 Gitee...
git push -u gitee main
if errorlevel 1 echo   失败: 请确认 %GITEE% 正确、仓库已建、公钥已添加到 Gitee。
echo.
echo 完成。
pause
