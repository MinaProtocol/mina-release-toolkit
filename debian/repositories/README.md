# Mina Protocol Debian Repository Pages

This directory contains HTML pages for Mina Protocol's Debian package repositories and a testing script.

## Files

### HTML Repository Pages
- `stable_packages_page.html` - Official stable Debian repository page
- `nightly_packages_page.html` - Nightly builds Debian repository page  
- `unstable_packages_page.html` - Unstable/development builds repository page

### Testing Script
- `test_from_html.sh` - Automated testing script that extracts installation commands from HTML files and validates them in Docker containers

## Usage

To test any of the HTML pages:

```bash
./test_from_html.sh --html stable_packages_page.html
./test_from_html.sh --html nightly_packages_page.html
./test_from_html.sh --html unstable_packages_page.html
```

For more options:
```bash
./test_from_html.sh --help
```

## Requirements

- Docker (for testing)
- Bash 4.0+ 
- Internet access