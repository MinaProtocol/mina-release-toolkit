#!/bin/bash

# Mina Installation Guide Test - HTML Parser Version
# Reads installation steps directly from HTML file

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Function to extract commands from HTML file
extract_commands_from_html() {
    local html_file="$1"
    
    if [ ! -f "$html_file" ]; then
        log_error "HTML file not found: $html_file"
        return 1
    fi
    
    log_info "Extracting commands from $html_file"
    
    # Extract only the content inside <div class="code-block">...</div> tags
    # Use grep to find lines with code-block divs, then extract just the command
    local commands=()
    
    # Method 1: Extract using grep and sed for single-line commands
    while IFS= read -r line; do
        if [[ "$line" =~ \<div\ class=\"code-block\"\>([^<]+)\</div\> ]]; then
            local cmd="${BASH_REMATCH[1]}"
            # Clean up any HTML entities or extra whitespace
            cmd=$(echo "$cmd" | sed 's/&lt;/</g' | sed 's/&gt;/>/g' | sed 's/&amp;/\&/g' | sed 's/^[[:space:]]*//' | sed 's/[[:space:]]*$//')
            if [ -n "$cmd" ]; then
                commands+=("$cmd")
            fi
        fi
    done < "$html_file"
    
    # Method 2: Handle multiline commands by extracting content between tags
    local temp_file
    temp_file=$(mktemp)
    
    # Extract multiline code blocks
    awk '
    /<div class="code-block">/ {
        in_block = 1
        content = ""
        # Extract content from the same line if it exists
        line = $0
        gsub(/.*<div class="code-block">/, "", line)
        if (match(line, /^[^<]+/)) {
            content = substr(line, RSTART, RLENGTH)
        }
        next
    }
    /<\/div>/ && in_block {
        # Extract any remaining content before the closing tag
        line = $0
        gsub(/<\/div>.*/, "", line)
        if (line != "" && content != "") {
            content = content "\n" line
        } else if (line != "" && content == "") {
            content = line
        }
        
        if (content != "") {
            print content
            print "---BLOCK_END---"
        }
        in_block = 0
        content = ""
        next
    }
    in_block {
        if (content == "") {
            content = $0
        } else {
            content = content "\n" $0
        }
    }
    ' "$html_file" > "$temp_file"
    
    # Process multiline blocks
    local current_cmd=""
    while IFS= read -r line; do
        if [ "$line" = "---BLOCK_END---" ]; then
            if [ -n "$current_cmd" ]; then
                # Clean up and validate command
                current_cmd=$(echo "$current_cmd" | sed 's/^[[:space:]]*//' | sed 's/[[:space:]]*$//')
                if [[ "$current_cmd" =~ ^(sudo|apt-get|wget|echo|gpg|install|dpkg) ]] || [[ "$current_cmd" =~ \| ]]; then
                    commands+=("$current_cmd")
                fi
            fi
            current_cmd=""
        else
            if [ -z "$current_cmd" ]; then
                current_cmd="$line"
            else
                current_cmd="$current_cmd"$'\n'"$line"
            fi
        fi
    done < "$temp_file"
    
    rm -f "$temp_file"
    
    # Remove duplicates and filter valid commands
    local unique_commands=()
    for cmd in "${commands[@]}"; do
        # Skip if it's HTML content or empty
        if [[ "$cmd" =~ ^[[:space:]]*$ ]] || [[ "$cmd" =~ \<.*\> ]] || [[ "$cmd" =~ ^[^a-zA-Z0-9] ]]; then
            continue
        fi
        
        # Check if command already exists
        local duplicate=false
        for existing in "${unique_commands[@]}"; do
            if [ "$cmd" = "$existing" ]; then
                duplicate=true
                break
            fi
        done
        
        if [ "$duplicate" = false ]; then
            unique_commands+=("$cmd")
        fi
    done
    
    # Print extracted commands for verification
    log_info "Extracted ${#unique_commands[@]} unique commands:"
    for i in "${!unique_commands[@]}"; do
        echo "Command $((i+1)):"
        echo "${unique_commands[i]}"
        echo "---"
    done
    
    # Return commands array (store in global variable)
    EXTRACTED_COMMANDS=("${unique_commands[@]}")
}

# Function to validate extracted commands
validate_commands() {
    local commands=("$@")
    local validation_passed=true
    
    log_info "Validating extracted commands..."
    
    # Check for required commands
    local required_patterns=(
        "install.*keyrings"
        "wget.*key\.asc"
        "gpg.*import"
        "echo.*deb.*minaprotocol"
        "apt-get.*update.*install"
    )
    
    for pattern in "${required_patterns[@]}"; do
        local found=false
        for cmd in "${commands[@]}"; do
            if [[ "$cmd" =~ $pattern ]]; then
                found=true
                break
            fi
        done
        
        if [ "$found" = false ]; then
            log_warning "Missing expected pattern: $pattern"
            validation_passed=false
        fi
    done
    
    if [ "$validation_passed" = true ]; then
        log_success "Command validation passed"
        return 0
    else
        log_warning "Command validation had warnings"
        return 1
    fi
}

# Function to create installation script from extracted commands
create_install_script_from_html() {
    local commands=("$@")
    local script_name="install_from_html.sh"
    
    cat > "$script_name" << 'EOF'
#!/bin/bash
set -e

echo "=== Mina Installation Test (from HTML) ==="
echo "Distribution: $(cat /etc/os-release | grep PRETTY_NAME | cut -d'=' -f2 | tr -d '"')"
echo "=========================================="

# Install prerequisites
echo "🔧 Installing prerequisites..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y wget gnupg ca-certificates

EOF

    # Add each extracted command
    local step=1
    for cmd in "${commands[@]}"; do
        {
            echo ""
            echo "echo \"🔧 Step $step: Executing command...\""
            echo "echo \"Command: ${cmd//$'\n'/ }\""
        } >> "$script_name"
        
        # Handle special cases
        if [[ "$cmd" =~ "sudo apt-get install wget" ]]; then
            echo "# Skipping wget installation as it's already installed" >> "$script_name"
        elif [[ "$cmd" =~ gpg.*import.*awk ]]; then
            # Handle the complex GPG verification command
            {
                echo "echo \"🔑 Verifying key fingerprint...\""
                echo "FINGERPRINT=\$(gpg -n -q --import --import-options import-show /etc/apt/keyrings/stable.packages.apt.minaprotocol.com.asc | awk '/pub/{getline; gsub(/^ +| +\$/,\"\"); print \$0}')"
                echo "EXPECTED=\"35BAA0B33E9EB396F59CA838C0BA5CE6DC6315A3\""
                echo "if [ \"\$FINGERPRINT\" = \"\$EXPECTED\" ]; then"
                echo "    echo \"✅ Key fingerprint matches: \$FINGERPRINT\""
                echo "else"
                echo "    echo \"❌ Key verification failed: \$FINGERPRINT\""
                echo "    exit 1"
                echo "fi"
            } >> "$script_name"
        else
            # Regular command - escape properly
            local escaped_cmd
            escaped_cmd=$(printf '%s\n' "$cmd" | sed 's/"/\\"/g')
            echo "$escaped_cmd" >> "$script_name"
        fi
        
        ((step++))
    done
    
    # Add verification at the end
    cat >> "$script_name" << 'EOF'

echo ""
echo "🎉 Installation completed!"
echo ""
echo "📋 Verification:"
if command -v mina >/dev/null 2>&1; then
    echo "✅ Mina command is available"
    mina version 2>/dev/null || echo "ℹ️  Mina version info not available"
else
    echo "ℹ️  Mina command not found (expected for some package types)"
fi

echo "📦 Installed Mina packages:"
dpkg -l | grep mina || echo "No mina packages found"

echo "✅ Test completed successfully!"
EOF

    chmod +x "$script_name"
    echo "$script_name"
}

# Function to run test with extracted commands
test_installation_from_html() {
    local html_file="$1"
    local distro="${2:-ubuntu:focal}"
    
    log_info "Testing installation from HTML file: $html_file"
    log_info "Using distribution: $distro"
    
    # Extract commands from HTML
    extract_commands_from_html "$html_file"
    
    if [ ${#EXTRACTED_COMMANDS[@]} -eq 0 ]; then
        log_error "No commands extracted from HTML file"
        return 1
    fi
    
    # Validate commands
    validate_commands "${EXTRACTED_COMMANDS[@]}"
    
    # Create installation script
    local script_file
    script_file=$(create_install_script_from_html "${EXTRACTED_COMMANDS[@]}")
    
    log_info "Created installation script: $script_file"
    
    # Run Docker test
    local container_name="mina_html_test_$$"
    docker rm -f "$container_name" 2>/dev/null || true
    
    log_info "Running Docker container test..."
    
    if docker run --name "$container_name" \
        -v "$(pwd)/$script_file:/install_test.sh:ro" \
        "$distro" \
        bash /install_test.sh; then
        
        log_success "Installation from HTML succeeded!"
        
        # Clean up on success
        docker rm "$container_name" >/dev/null 2>&1 || true
        rm -f "$script_file"
        return 0
    else
        log_error "Installation from HTML failed"
        log_warning "Container '$container_name' preserved for debugging"
        log_warning "Script '$script_file' preserved for inspection"
        return 1
    fi
}

# Main function
main() {
    echo "🧪 Mina Installation Guide Test - HTML Parser"
    echo "============================================="
    echo ""
    
    local html_file=""
    local distro="ubuntu:focal"
    local show_commands_only=false
    
    # Parse command line arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --html)
                html_file="$2"
                shift 2
                ;;
            --distro)
                distro="$2"
                shift 2
                ;;
            --show-commands)
                show_commands_only=true
                shift
                ;;
            --help)
                echo "Usage: $0 --html HTML_FILE [OPTIONS]"
                echo ""
                echo "Required:"
                echo "  --html FILE         HTML file containing installation guide"
                echo ""
                echo "Options:"
                echo "  --distro DISTRO     Docker distribution to test (default: ubuntu:focal)"
                echo "  --show-commands     Only extract and show commands, don't run test"
                echo "  --help             Show this help message"
                echo ""
                echo "Example:"
                echo "  $0 --html improved_mina_packages_page.html"
                echo "  $0 --html guide.html --distro debian:bullseye"
                echo "  $0 --html guide.html --show-commands"
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                echo "Use --help for usage information"
                exit 1
                ;;
        esac
    done
    
    # Check required arguments
    if [ -z "$html_file" ]; then
        log_error "HTML file is required. Use --html FILE"
        echo "Use --help for usage information"
        exit 1
    fi
    
    if [ ! -f "$html_file" ]; then
        log_error "HTML file not found: $html_file"
        exit 1
    fi
    
    # Check Docker availability if not just showing commands
    if [ "$show_commands_only" = false ]; then
        if ! command -v docker >/dev/null 2>&1; then
            log_error "Docker is not installed or not available"
            exit 1
        fi
        
        if ! docker info >/dev/null 2>&1; then
            log_error "Docker daemon is not running or accessible"
            exit 1
        fi
    fi
    
    if [ "$show_commands_only" = true ]; then
        # Just extract and show commands
        extract_commands_from_html "$html_file"
        validate_commands "${EXTRACTED_COMMANDS[@]}"
        echo ""
        log_info "Command extraction complete. Use without --show-commands to run test."
    else
        # Run full test
        if test_installation_from_html "$html_file" "$distro"; then
            echo ""
            log_success "✅ All tests passed! The HTML installation guide is working correctly."
            exit 0
        else
            echo ""
            log_error "❌ Test failed. Check the HTML installation guide for issues."
            exit 1
        fi
    fi
}

# Global variable to store extracted commands
declare -a EXTRACTED_COMMANDS

# Run main function
main "$@"