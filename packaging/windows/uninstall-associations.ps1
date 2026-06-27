$ErrorActionPreference = "Stop"
$classes = "HKCU:\Software\Classes"
$progId = "Mado.SourceFile"
$extensions = @(
    ".md", ".markdown", ".txt", ".py", ".rs", ".js", ".jsx", ".ts", ".tsx",
    ".json", ".toml", ".yaml", ".yml", ".lua", ".c", ".h", ".cpp", ".hpp",
    ".go", ".java", ".rb", ".sh"
)

Remove-Item (Join-Path $classes $progId) -Recurse -Force -ErrorAction SilentlyContinue
foreach ($extension in $extensions) {
    $openWith = Join-Path $classes "$extension\OpenWithProgids"
    Remove-ItemProperty -Path $openWith -Name $progId -Force -ErrorAction SilentlyContinue
}

Write-Host "Removed Mado file associations."

