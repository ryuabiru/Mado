param(
    [string]$Executable = (Join-Path $PSScriptRoot "../../target/release/mado.exe")
)

$ErrorActionPreference = "Stop"
$exe = (Resolve-Path $Executable).Path
$classes = "HKCU:\Software\Classes"
$progId = "Mado.SourceFile"
$progKey = Join-Path $classes $progId
$extensions = @(
    ".md", ".markdown", ".txt", ".py", ".rs", ".js", ".jsx", ".ts", ".tsx",
    ".json", ".toml", ".yaml", ".yml", ".lua", ".c", ".h", ".cpp", ".hpp",
    ".go", ".java", ".rb", ".sh"
)

New-Item -Path $progKey -Force | Out-Null
Set-Item -Path $progKey -Value "Mado source file"
New-Item -Path (Join-Path $progKey "DefaultIcon") -Force | Out-Null
Set-Item -Path (Join-Path $progKey "DefaultIcon") -Value "`"$exe`",0"
New-Item -Path (Join-Path $progKey "shell/open/command") -Force | Out-Null
Set-Item -Path (Join-Path $progKey "shell/open/command") -Value "`"$exe`" `"%1`""

foreach ($extension in $extensions) {
    $openWith = Join-Path $classes "$extension\OpenWithProgids"
    New-Item -Path $openWith -Force | Out-Null
    New-ItemProperty -Path $openWith -Name $progId -PropertyType String -Value "" -Force | Out-Null
}

Write-Host "Registered Mado in Open With for $($extensions.Count) file types."
Write-Host "Choose Mado through Windows Settings to make it the default application."

