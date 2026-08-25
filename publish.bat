@echo off
chcp 65001 >nul
cd /d "%~dp0"
set "GH=https://github.com/YOUR_USER/ContentSonar.git"
set "GITEE=https://gitee.com/YOUR_USER/ContentSonar.git"
echo === ContentSonar 发布 (GitHub + Gitee) ===
echo 请先改本文件顶部 GH / GITEE 为你的仓库地址，并在 GitHub/Gitee 建好同名仓库。
echo.
if not exist .git ( git init )
git branch -M main
git add -A
git commit -m "init: ContentSonar"
git remote get-url origin >nul 2>nul || git remote add origin %GH%
git remote get-url gitee >nul 2>nul || git remote add gitee %GITEE%
echo 推送 GitHub...
git push -u origin main
if errorlevel 1 echo   若失败：仓库可能已有内容，需先 pull 或 git push -f。
echo 推送 Gitee...
git push -u gitee main
if errorlevel 1 echo   若失败：同上，或检查 Gitee 账号权限。
echo.
echo 完成。pause
