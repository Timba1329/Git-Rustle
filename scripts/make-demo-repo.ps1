<#
.SYNOPSIS
  Creates a fictional shop project ("acme-shop") with several authors, branches, merges, folders and
  local changes. Used for the screenshots in the README and handy for trying Rustle by hand.

.EXAMPLE
  .\scripts\make-demo-repo.ps1 -Path C:\temp\acme-shop
#>
param([string]$Path = (Join-Path $env:TEMP 'acme-shop'))

$ErrorActionPreference = 'Stop'
if (Test-Path $Path) { throw "$Path already exists" }
$remote = "$Path-remote.git"

git init -q --bare -b main $remote
New-Item -ItemType Directory $Path | Out-Null
Set-Location $Path
git init -q -b main
git config commit.gpgsign false
git config core.autocrlf false

$people = @(
    @('Alex Morgan', 'alex@acme.example'),
    @('Sam Rivera', 'sam@acme.example'),
    @('Jo Kim', 'jo@acme.example')
)
$script:hours = 24 * 40
$script:n = 0

function Save([string]$file, [string]$text) {
    $dir = Split-Path $file -Parent
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory $dir | Out-Null }
    Set-Content -Path $file -Value $text
}

function Commit([string]$message) {
    $p = $people[$script:n % $people.Count]
    $script:n++
    $script:hours = [Math]::Max(1, $script:hours - 7)
    $when = (Get-Date).AddHours(-$script:hours).ToString('s')
    $env:GIT_AUTHOR_NAME = $p[0]; $env:GIT_AUTHOR_EMAIL = $p[1]
    $env:GIT_COMMITTER_NAME = $p[0]; $env:GIT_COMMITTER_EMAIL = $p[1]
    $env:GIT_AUTHOR_DATE = $when; $env:GIT_COMMITTER_DATE = $when
    git add -A
    git commit -q -m $message
}

function Merge([string]$branch, [string]$into) {
    $p = $people[$script:n % $people.Count]
    $script:n++
    $script:hours = [Math]::Max(1, $script:hours - 5)
    $when = (Get-Date).AddHours(-$script:hours).ToString('s')
    $env:GIT_AUTHOR_NAME = $p[0]; $env:GIT_AUTHOR_EMAIL = $p[1]
    $env:GIT_COMMITTER_NAME = $p[0]; $env:GIT_COMMITTER_EMAIL = $p[1]
    $env:GIT_AUTHOR_DATE = $when; $env:GIT_COMMITTER_DATE = $when
    git merge -q --no-ff $branch -m "Merge branch '$branch' into $into"
}

Save 'README.md' "# Acme Shop`nA small demo shop."
Commit 'Initial commit'
Save 'src/Catalog.php' "<?php`nclass Catalog {`n    public function all(): array { return []; }`n}"
Save 'resources/lang/en/shop.php' "<?php`nreturn ['cart' => 'Cart', 'checkout' => 'Checkout'];"
Commit 'Add catalog and English texts'
Save 'resources/lang/nl/shop.php' "<?php`nreturn ['cart' => 'Winkelwagen', 'checkout' => 'Afrekenen'];"
Save 'resources/lang/de/shop.php' "<?php`nreturn ['cart' => 'Warenkorb', 'checkout' => 'Kasse'];"
Commit 'Translate the shop texts'
Save 'src/Cart.php' "<?php`nclass Cart {`n    private array `$items = [];`n    public function add(string `$sku): void { `$this->items[] = `$sku; }`n    public function count(): int { return count(`$this->items); }`n}"
Save 'tests/CartTest.php' "<?php`nclass CartTest { public function testAdd() { /* ... */ } }"
Commit 'Add the cart'
git branch develop
git switch -q develop

git switch -q -c feature/cart
Save 'src/Cart.php' "<?php`nclass Cart {`n    private array `$items = [];`n    public function add(string `$sku, int `$qty = 1): void { `$this->items[`$sku] = (`$this->items[`$sku] ?? 0) + `$qty; }`n    public function count(): int { return array_sum(`$this->items); }`n    public function remove(string `$sku): void { unset(`$this->items[`$sku]); }`n}"
Commit 'Cart: support quantities'
Save 'tests/CartTest.php' "<?php`nclass CartTest {`n    public function testAdd() { /* ... */ }`n    public function testQuantities() { /* ... */ }`n    public function testRemove() { /* ... */ }`n}"
Commit 'Cart: tests for quantities and removal'
git switch -q develop

git switch -q -c feature/checkout
Save 'src/Checkout.php' "<?php`nclass Checkout {`n    public function total(Cart `$cart): float { return 0.0; }`n}"
Commit 'Add checkout skeleton'
Save 'src/Checkout.php' "<?php`nclass Checkout {`n    public function total(Cart `$cart): float { return `$cart->count() * 9.99; }`n    public function vat(float `$total): float { return round(`$total * 0.21, 2); }`n}"
Commit 'Checkout: total and VAT'
git switch -q develop
Merge 'feature/cart' 'develop'

git switch -q -c Tickets/SHOP-12
Save 'docs/shipping.md' "# Shipping`nOrders ship within 2 days."
Commit 'SHOP-12: document shipping'
git switch -q develop
git switch -q -c Tickets/SHOP-18
Save 'resources/lang/en/shop.php' "<?php`nreturn ['cart' => 'Cart', 'checkout' => 'Checkout', 'empty' => 'Your cart is empty'];"
Commit 'SHOP-18: empty cart text'
git switch -q develop
Merge 'feature/checkout' 'develop'
Merge 'Tickets/SHOP-12' 'develop'

git switch -q -c fix/login-bug
Save 'src/Login.php' "<?php`nclass Login { public function attempt(string `$u, string `$p): bool { return false; } }"
Commit 'Fix login attempts'
git switch -q develop
Merge 'Tickets/SHOP-18' 'develop'
Merge 'fix/login-bug' 'develop'

git switch -q main
Merge 'develop' 'main'
git tag v1.0
git switch -q develop
Save 'docs/changelog.md' "# Changelog`n- Cart quantities`n- Checkout and VAT"
Commit 'Document the release'

git remote add origin $remote
git push -q -u origin main develop feature/cart feature/checkout fix/login-bug Tickets/SHOP-12 Tickets/SHOP-18 --tags

# Local changes to play with
Remove-Item Env:GIT_AUTHOR_DATE, Env:GIT_COMMITTER_DATE -ErrorAction SilentlyContinue
Save 'src/Cart.php' "<?php`nclass Cart {`n    private array `$items = [];`n    public function add(string `$sku, int `$qty = 1): void { `$this->items[`$sku] = (`$this->items[`$sku] ?? 0) + `$qty; }`n    public function count(): int { return array_sum(`$this->items); }`n    public function remove(string `$sku): void { unset(`$this->items[`$sku]); }`n    public function clear(): void { `$this->items = []; }`n    public function isEmpty(): bool { return `$this->count() === 0; }`n}"
Save 'resources/lang/nl/shop.php' "<?php`nreturn ['cart' => 'Winkelwagen', 'checkout' => 'Afrekenen', 'empty' => 'Je winkelwagen is leeg'];"
git add resources/lang/nl/shop.php
Save 'src/Coupon.php' "<?php`nclass Coupon { public function __construct(public string `$code, public float `$percent) {} }"
Remove-Item tests/CartTest.php
Write-Host "Created $Path (remote: $remote)"
