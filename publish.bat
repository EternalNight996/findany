@echo off
chcp 65001 >nul
cd /d "%~dp0"
set "GH=git@github.com:YOUR_USER/findany.git"
set "GITEE=git@gitee.com:YOUR_USER/findany.git"
echo === findany 发布（SSH 推送到 GitHub + Gitee）===
echo 请先改本文件顶部 GH / GITEE 为你的 SSH 地址，并在两平台建好同名空仓库。
echo 需先配置 SSH 公钥（见 README「发布」节：ssh-keygen + 平台添加公钥）。
echo.
if not exist .git ( git init )
git branch -M main
git add -A
git commit -m "init: findany"
git remote get-url origin >nul 2>nul || git remote add origin %GH%
git remote get-url gitee >nul 2>nul || git remote add gitee %GITEE%
echo 推送 GitHub...
git push -u origin main
if errorlevel 1 echo   若失败：仓库已有内容需先 pull，或检查 SSH 密钥/权限。
echo 推送 Gitee...
git push -u gitee main
if errorlevel 1 echo   若失败：同上，或检查 Gitee SSH 密钥。
echo.
echo 完成。pause
