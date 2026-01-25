# AutoContinue Installation Guide for LLM Agents

You are helping a user install AutoContinue (AC), a CLI wrapper tool for automatic continuation and retry of AI CLI tools.

## Option 1: Direct Download (Recommended)

Download pre-built binaries from GitHub Releases:

```bash
# Get the latest release URL
# https://github.com/MoYeRanQianZhi/AutoContinue/releases/latest
```

### For Windows (x64):
```powershell
# Download and extract
Invoke-WebRequest -Uri "https://github.com/MoYeRanQianZhi/AutoContinue/releases/latest/download/ac-vX.X.Xx86_64-pc-windows-msvc.zip" -OutFile ac.zip
Expand-Archive ac.zip -DestinationPath .
Copy-Item ac.exe $env:USERPROFILE\.cargo\bin\
Remove-Item ac.zip
```

### For Linux (x64):
```bash
# Download and extract
curl -LO https://github.com/MoYeRanQianZhi/AutoContinue/releases/latest/download/ac-vX.X.Xx86_64-unknown-linux-gnu.tar.gz
tar -xzf ac-vX.X.Xx86_64-unknown-linux-gnu.tar.gz
sudo cp ac /usr/local/bin/
chmod +x /usr/local/bin/ac
rm ac-vX.X.Xx86_64-unknown-linux-gnu.tar.gz
```

### For macOS (Intel x64):
```bash
curl -LO https://github.com/MoYeRanQianZhi/AutoContinue/releases/latest/download/ac-vX.X.Xx86_64-apple-darwin.tar.gz
tar -xzf ac-vX.X.Xx86_64-apple-darwin.tar.gz
sudo cp ac /usr/local/bin/
chmod +x /usr/local/bin/ac
rm ac-vX.X.Xx86_64-apple-darwin.tar.gz
```

### For macOS (Apple Silicon ARM64):
```bash
curl -LO https://github.com/MoYeRanQianZhi/AutoContinue/releases/latest/download/ac-vX.X.Xaarch64-apple-darwin.tar.gz
tar -xzf ac-vX.X.Xaarch64-apple-darwin.tar.gz
sudo cp ac /usr/local/bin/
chmod +x /usr/local/bin/ac
rm ac-vX.X.Xaarch64-apple-darwin.tar.gz
```

---

## Option 2: Build from Source

Use this method if pre-built binaries are not available or you need a custom build.

### System Requirements

- Git
- Rust toolchain (cargo) version 1.70+

### Installation Steps

### Step 1: Check if Rust is installed

```bash
cargo --version
```

If Rust is not installed, install it first:

**Windows (PowerShell):**
```powershell
Invoke-WebRequest -Uri https://win.rustup.rs/x86_64 -OutFile rustup-init.exe
.\rustup-init.exe -y
Remove-Item rustup-init.exe
# Restart terminal or run: $env:PATH += ";$env:USERPROFILE\.cargo\bin"
```

**Linux/macOS:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
```

### Step 2: Clone and build AutoContinue

```bash
git clone https://github.com/MoYeRanQianZhi/AutoContinue.git
cd AutoContinue
cargo build --release
```

### Step 3: Install the binary

**Windows:**
```powershell
Copy-Item target\release\ac.exe $env:USERPROFILE\.cargo\bin\
```

**Linux/macOS:**
```bash
cp target/release/ac ~/.cargo/bin/
```

### Step 4: Verify installation

```bash
ac --version
```

Expected output: `ac x.x.x` (version number)

## Usage Examples

After installation, the user can use AC like this:

```bash
# Basic usage with Claude
ac claude --resume -cp "continue" -rp "retry"

# With custom prompts
ac claude --resume -cp "Please continue the task" -rp "Please retry"

# With other AI CLIs
ac codex -cp "continue"
ac opencode -cp "继续"
```

## Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `-cp, --continue-prompt` | Continue prompt | "继续" |
| `-rp, --retry-prompt` | Retry prompt | "重试" |
| `-st, --sleep-time` | Extra wait time (seconds) | 15 |
| `-sth, --silence-threshold` | Silence threshold (seconds) | 30 |

## Troubleshooting

1. **"cargo not found"**: Install Rust first using the commands above
2. **"ac not found"**: Ensure `~/.cargo/bin` is in PATH
3. **Build errors**: Ensure Rust version >= 1.70 (`rustup update`)

## Success Criteria

Installation is successful when `ac --version` outputs a version number.
