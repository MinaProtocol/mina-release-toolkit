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
    
    # Extract commands from code-block divs that may contain copy buttons
    local temp_file
    temp_file=$(mktemp)
    
    # Use awk to extract content between <div class="code-block"> and <button class="copy-button">
    # or between <div class="code-block"> and </div> if no button present
    awk '
    /<div class="code-block"[^>]*>/ {
        in_block = 1
        content = ""
        # Extract content from the same line if it exists
        line = $0
        gsub(/.*<div class="code-block"[^>]*>/, "", line)
        # Remove any trailing button or closing div tags from the same line
        gsub(/<button.*/, "", line)
        gsub(/<\/div>.*/, "", line)
        if (line != "") {
            content = line
        }
        next
    }
    /<button class="copy-button"/ && in_block {
        # End of command, output what we have
        if (content != "") {
            gsub(/^[[:space:]]*/, "", content)  # Remove leading whitespace
            gsub(/[[:space:]]*$/, "", content)  # Remove trailing whitespace
            if (content != "") {
                print content
            }
        }
        in_block = 0
        content = ""
        next
    }
    /<\/div>/ && in_block {
        # End of command block without button
        line = $0
        gsub(/<\/div>.*/, "", line)
        if (line != "" && content != "") {
            content = content "\n" line
        } else if (line != "" && content == "") {
            content = line
        }
        
        if (content != "") {
            gsub(/^[[:space:]]*/, "", content)  # Remove leading whitespace
            gsub(/[[:space:]]*$/, "", content)  # Remove trailing whitespace
            if (content != "") {
                print content
            }
        }
        in_block = 0
        content = ""
        next
    }
    in_block {
        # Accumulate content within the block
        if (content == "") {
            content = $0
        } else {
            content = content "\n" $0
        }
    }
    ' "$html_file" > "$temp_file"
    
    # Read commands from temp file and clean them up
    local commands=()
    while IFS= read -r line; do
        if [ -n "$line" ]; then
            # Clean up HTML entities and whitespace
            line=$(echo "$line" | sed 's/&lt;/</g' | sed 's/&gt;/>/g' | sed 's/&amp;/\&/g' | sed 's/&quot;/"/g')
            line=$(echo "$line" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
            
            # Skip empty lines, comments, or lines that are just HTML artifacts
            if [[ -n "$line" && ! "$line" =~ ^[[:space:]]*# && ! "$line" =~ ^[[:space:]]*$ && ! "$line" =~ ^\<.*\>$ ]]; then
                commands+=("$line")
            fi
        fi
    done < "$temp_file"
    
    rm -f "$temp_file"
    
    # Remove duplicates while preserving order
    local unique_commands=()
    for cmd in "${commands[@]}"; do
        # Skip lsb_release commands as they're informational
        if [[ "$cmd" =~ lsb_release ]]; then
            continue
        fi
        
        # Check if command is already in the array
        local found=0
        for existing in "${unique_commands[@]}"; do
            if [ "$existing" = "$cmd" ]; then
                found=1
                break
            fi
        done
        if [ $found -eq 0 ]; then
            unique_commands+=("$cmd")
        fi
    done
    
    # Print extracted commands
    log_info "Extracted ${#unique_commands[@]} unique commands:"
    for i in "${!unique_commands[@]}"; do
        echo "  $((i+1)). ${unique_commands[i]}"
    done
    
    if [ ${#unique_commands[@]} -eq 0 ]; then
        log_error "No commands extracted from HTML file"
        return 1
    fi
    
    # Store commands in global array for use by test function
    EXTRACTED_COMMANDS=("${unique_commands[@]}")
    return 0
}

# Function to validate extracted commands
validate_commands() {
    local commands=("$@")
    local validation_passed=true
    
    log_info "Validating ${#commands[@]} commands..."
    
    for i in "${!commands[@]}"; do
        local cmd="${commands[i]}"
        local cmd_num=$((i+1))
        
        # Basic syntax validation
        if [[ -z "$cmd" ]]; then
            log_error "Command $cmd_num is empty"
            validation_passed=false
            continue
        fi
        
        # Check for suspicious patterns
        if [[ "$cmd" =~ \;\s*rm\s+-rf|sudo\s+rm\s+-rf ]]; then
            log_error "Command $cmd_num contains dangerous rm -rf: $cmd"
            validation_passed=false
        fi
        
        # Check for basic shell syntax
        if [[ "$cmd" =~ ^[a-zA-Z] ]]; then
            log_info "✓ Command $cmd_num looks valid: ${cmd:0:60}..."
        else
            log_warning "? Command $cmd_num might have syntax issues: $cmd"
        fi
    done
    
    if [ "$validation_passed" = true ]; then
        log_success "All commands passed basic validation"
        return 0
    else
        log_error "Some commands failed validation"
        return 1
    fi
}

# Function to test commands in Docker container
test_commands_in_container() {
    local distribution="$1"
    local commands=("${@:2}")
    
    log_info "Testing commands in Docker container ($distribution)..."
    
    # Create a temporary script with all commands
    local script_file
    script_file=$(mktemp)
    
    # Add shebang and error handling
    cat > "$script_file" << 'EOF'
#!/bin/bash
set -e
export DEBIAN_FRONTEND=noninteractive
EOF
    
    # Add each command to the script
    for cmd in "${commands[@]}"; do
        echo "$cmd" >> "$script_file"
    done
    
    # Add final verification
    cat >> "$script_file" << 'EOF'
echo "Installation completed successfully!"
EOF
    
    chmod +x "$script_file"
    
    log_info "Created test script with ${#commands[@]} commands"
    log_info "Running Docker container test..."
    
    # Run the script in Docker container
    if docker run --rm -v "$script_file:/test_script.sh" "$distribution" /bin/bash -c "/test_script.sh"; then
        log_success "✅ All commands executed successfully in $distribution container"
        rm -f "$script_file"
        return 0
    else
        log_error "❌ Command execution failed in $distribution container"
        log_info "Test script saved at: $script_file (for debugging)"
        return 1
    fi
}

# Function to run syntax check on commands
syntax_check_commands() {
    local commands=("$@")
    
    log_info "Performing syntax check on commands..."
    
    for i in "${!commands[@]}"; do
        local cmd="${commands[i]}"
        local cmd_num=$((i+1))
        
        # Create a temporary script to test syntax
        local temp_script
        temp_script=$(mktemp)
        echo "#!/bin/bash" > "$temp_script"
        echo "$cmd" >> "$temp_script"
        
        if bash -n "$temp_script" 2>/dev/null; then
            log_info "✓ Command $cmd_num syntax OK"
        else
            log_error "✗ Command $cmd_num syntax error: $cmd"
            rm -f "$temp_script"
            return 1
        fi
        
        rm -f "$temp_script"
    done
    
    log_success "All commands passed syntax check"
    return 0
}

# Main function to test HTML file
test_html_file() {
    local html_file="$1"
    local distribution="${2:-ubuntu:focal}"
    local run_docker_test="${3:-false}"
    
    echo "🧪 Mina Installation Guide Test - HTML Parser"
    echo "============================================="
    echo
    
    log_info "Testing installation from HTML file: $html_file"
    log_info "Using distribution: $distribution"
    
    # Extract commands from HTML
    if ! extract_commands_from_html "$html_file"; then
        log_error "❌ Failed to extract commands from HTML file"
        return 1
    fi
    
    # Validate commands
    if ! validate_commands "${EXTRACTED_COMMANDS[@]}"; then
        log_error "❌ Command validation failed"
        return 1
    fi
    
    # Syntax check
    if ! syntax_check_commands "${EXTRACTED_COMMANDS[@]}"; then
        log_error "❌ Syntax check failed"
        return 1
    fi
    
    # Docker test (optional)
    if [ "$run_docker_test" = "true" ]; then
        if command -v docker >/dev/null 2>&1; then
            if ! test_commands_in_container "$distribution" "${EXTRACTED_COMMANDS[@]}"; then
                log_error "❌ Docker container test failed"
                return 1
            fi
        else
            log_warning "Docker not available, skipping container test"
        fi
    else
        log_info "Skipping Docker test (use --docker to enable)"
    fi
    
    log_success "✅ All tests passed!"
    return 0
}

# Parse command line arguments
usage() {
    echo "Usage: $0 --html <html_file> [--distribution <dist>] [--docker]"
    echo ""
    echo "Options:"
    echo "  --html <file>           HTML file to test"
    echo "  --distribution <dist>   Docker distribution to test with (default: ubuntu:focal)"
    echo "  --docker               Run actual Docker test (default: false)"
    echo ""
    echo "Examples:"
    echo "  $0 --html stable_packages_page.html"
    echo "  $0 --html nightly_packages_page.html --distribution ubuntu:noble"
    echo "  $0 --html unstable_packages_page.html --docker"
}

# Global variables
EXTRACTED_COMMANDS=()
HTML_FILE=""
DISTRIBUTION="ubuntu:focal"
RUN_DOCKER_TEST=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --html)
            HTML_FILE="$2"
            shift 2
            ;;
        --distribution)
            DISTRIBUTION="$2"
            shift 2
            ;;
        --docker)
            RUN_DOCKER_TEST=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# Check required arguments
if [ -z "$HTML_FILE" ]; then
    log_error "HTML file is required"
    usage
    exit 1
fi

# Run the test
if test_html_file "$HTML_FILE" "$DISTRIBUTION" "$RUN_DOCKER_TEST"; then
    echo
    log_success "🎉 Test completed successfully!"
    exit 0
else
    echo
    log_error "❌ Test failed. Check the HTML installation guide for issues."
    exit 1
fi