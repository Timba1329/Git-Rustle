<#
.SYNOPSIS
  Creates a small sample repository (with a remote, branches, a merge and local changes) for
  trying out Rustle by hand.

.EXAMPLE
  .\scripts\make-test-repo.ps1 -Path C:\temp\rustle-demo
#>
param([string]$Path = (Join-Path $env:TEMP 'rustle-demo'))

$ErrorActionPreference = 'Stop'
if (Test-Path $Path) { throw "$Path already exists" }
$remote = "$Path-remote.git"

git init -q --bare -b main $remote
New-Item -ItemType Directory $Path | Out-Null
Set-Location $Path
git init -q -b main
git config user.name 'Demo User'
git config user.email 'demo@example.com'
git config commit.gpgsign false

"# Demo" | Set-Content README.md
git add .; git commit -qm 'Initial commit'
1..30 | ForEach-Object { "line $_" | Add-Content data.txt; git add .; git commit -qm "Update $_" }

git switch -q -c feature/x
'feature' | Set-Content feature.txt; git add .; git commit -qm 'Add feature X'
git switch -q main
'hotfix' | Set-Content hotfix.txt; git add .; git commit -qm 'Hotfix on main'
git merge -q --no-ff feature/x -m 'Merge feature/x'
git tag v0.1
git remote add origin $remote
git push -q -u origin main feature/x --tags

# Local changes to play with
Add-Content data.txt 'changed locally'
'staged' | Add-Content README.md; git add README.md
'new file' | Set-Content untracked.txt
Write-Host "Created $Path (remote: $remote)"
