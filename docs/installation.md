# Installation Guide

## Prerequisites

FORGE requires two main dependencies:

1. **[Rust](https://www.rust-lang.org/tools/install)** (1.85.0 or newer)
2. **[Docker](https://docs.docker.com/get-docker/)** (20.10.0 or newer)

### Installing Docker

#### Windows
1. Download [Docker Desktop for Windows](https://docs.docker.com/desktop/install/windows-install/)
2. Run the installer and follow the setup wizard
3. Restart your computer
4. Verify installation: `docker --version`

**With Package Manager:**
```powershell
# With Chocolatey
choco install docker-desktop
```

#### macOS
1. Download [Docker Desktop for Mac](https://docs.docker.com/desktop/install/mac-install/)
2. Drag Docker.app to Applications folder
3. Launch Docker Desktop
4. Verify installation: `docker --version`

**With Package Manager:**
```bash
# With Homebrew
brew install --cask docker
```

#### Linux (Ubuntu/Debian)
```bash
# Update package index
sudo apt update

# Install Docker
sudo apt install docker.io

# Start Docker service
sudo systemctl start docker
sudo systemctl enable docker

# Add your user to docker group (optional, to run without sudo)
sudo usermod -aG docker $USER

# Verify installation
docker --version
```

### Installing Rust

```bash
# Install Rust using rustup (all platforms)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# On Windows, download from: https://rustup.rs/

# Verify installation
rustc --version
cargo --version
```

## Installing FORGE

Once Docker and Rust are installed, you can install FORGE:

### From Source (Recommended)

```bash
# Clone the repository
git clone https://github.com/0xReLogic/Forge.git
cd Forge

# Build the project
cargo build --release

# The binary will be at: target/release/forge
```

### With Cargo

```bash
cargo install forge
```

### Pre-compiled Binaries

Download from the [releases page](https://github.com/0xReLogic/Forge/releases).

## Verification

After installation, verify everything is working:

```bash
# Check FORGE version
forge --version

# Check Docker is running
docker ps

# Check Rust installation
cargo --version
```

## Optional: VSCode Setup

If you use VSCode, these extensions can improve your workflow:
- **YAML** (redhat.vscode-yaml) - Syntax highlighting for forge.yaml
- **Docker** (ms-azuretools.vscode-docker) - Docker management

**Note**: These extensions are optional. FORGE works from any terminal.
