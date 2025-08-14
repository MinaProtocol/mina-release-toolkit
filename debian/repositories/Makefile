# Makefile for deploying Mina Protocol Debian repository pages

# Variables
TEMP_DIR := temp_deploy
AWS_REGION := us-west-2
AWS_PROFILE := default

# CloudFront Distribution IDs
STABLE_CLOUDFRONT_ID := E8IPE9Z3UQN83
NIGHTLY_CLOUDFRONT_ID := EWBQHX325L7M0
UNSTABLE_CLOUDFRONT_ID := ESAC44KCOWHFM

# S3 bucket names
STABLE_BUCKET := stable.apt.packages.minaprotocol.com
NIGHTLY_BUCKET := nightly.apt.packages.minaprotocol.com
UNSTABLE_BUCKET := unstable.apt.packages.minaprotocol.com

# Source files
STABLE_HTML := stable_packages_page.html
NIGHTLY_HTML := nightly_packages_page.html
UNSTABLE_HTML := unstable_packages_page.html

# Test script
TEST_SCRIPT := ./test_from_html.sh

# Default target
.PHONY: all
all: deploy-stable deploy-nightly deploy-unstable

# Deploy stable repository page
.PHONY: deploy-stable
deploy-stable:
	@echo "Deploying stable repository page to $(STABLE_BUCKET)..."
	@mkdir -p $(TEMP_DIR)/stable
	@cp $(STABLE_HTML) $(TEMP_DIR)/stable/index.html
	@aws s3 cp $(TEMP_DIR)/stable/index.html s3://$(STABLE_BUCKET)/index.html \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) \
		--acl public-read \
		--cache-control "public, max-age=3600"
	@echo "Invalidating CloudFront cache for $(STABLE_CLOUDFRONT_ID)..."
	@aws cloudfront create-invalidation \
		--distribution-id $(STABLE_CLOUDFRONT_ID) \
		--paths "/index.html" "/" \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) > /dev/null
	@rm -rf $(TEMP_DIR)/stable
	@echo "✅ Stable repository page deployed and cache invalidated successfully"

# Deploy nightly repository page
.PHONY: deploy-nightly
deploy-nightly:
	@echo "Deploying nightly repository page to $(NIGHTLY_BUCKET)..."
	@mkdir -p $(TEMP_DIR)/nightly
	@cp $(NIGHTLY_HTML) $(TEMP_DIR)/nightly/index.html
	@aws s3 cp $(TEMP_DIR)/nightly/index.html s3://$(NIGHTLY_BUCKET)/index.html \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) \
		--acl public-read \
		--cache-control "public, max-age=1800"
	@echo "Invalidating CloudFront cache for $(NIGHTLY_CLOUDFRONT_ID)..."
	@aws cloudfront create-invalidation \
		--distribution-id $(NIGHTLY_CLOUDFRONT_ID) \
		--paths "/index.html" "/" \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) > /dev/null
	@rm -rf $(TEMP_DIR)/nightly
	@echo "✅ Nightly repository page deployed and cache invalidated successfully"

# Deploy unstable repository page
.PHONY: deploy-unstable
deploy-unstable:
	@echo "Deploying unstable repository page to $(UNSTABLE_BUCKET)..."
	@mkdir -p $(TEMP_DIR)/unstable
	@cp $(UNSTABLE_HTML) $(TEMP_DIR)/unstable/index.html
	@aws s3 cp $(TEMP_DIR)/unstable/index.html s3://$(UNSTABLE_BUCKET)/index.html \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) \
		--acl public-read \
		--cache-control "public, max-age=900"
	@echo "Invalidating CloudFront cache for $(UNSTABLE_CLOUDFRONT_ID)..."
	@aws cloudfront create-invalidation \
		--distribution-id $(UNSTABLE_CLOUDFRONT_ID) \
		--paths "/index.html" "/" \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) > /dev/null
	@rm -rf $(TEMP_DIR)/unstable
	@echo "✅ Unstable repository page deployed and cache invalidated successfully"

# Test stable repository page
.PHONY: test-stable
test-stable:
	@echo "Testing stable repository page..."
	@test -f $(STABLE_HTML) || (echo "❌ $(STABLE_HTML) not found" && exit 1)
	@test -x $(TEST_SCRIPT) || (echo "❌ $(TEST_SCRIPT) not found or not executable" && exit 1)
	@$(TEST_SCRIPT) --html $(STABLE_HTML)
	@echo "✅ Stable repository page test completed"

# Test nightly repository page
.PHONY: test-nightly
test-nightly:
	@echo "Testing nightly repository page..."
	@test -f $(NIGHTLY_HTML) || (echo "❌ $(NIGHTLY_HTML) not found" && exit 1)
	@test -x $(TEST_SCRIPT) || (echo "❌ $(TEST_SCRIPT) not found or not executable" && exit 1)
	@$(TEST_SCRIPT) --html $(NIGHTLY_HTML)
	@echo "✅ Nightly repository page test completed"

# Test unstable repository page
.PHONY: test-unstable
test-unstable:
	@echo "Testing unstable repository page..."
	@test -f $(UNSTABLE_HTML) || (echo "❌ $(UNSTABLE_HTML) not found" && exit 1)
	@test -x $(TEST_SCRIPT) || (echo "❌ $(TEST_SCRIPT) not found or not executable" && exit 1)
	@$(TEST_SCRIPT) --html $(UNSTABLE_HTML)
	@echo "✅ Unstable repository page test completed"

# Test all repository pages
.PHONY: test
test: test-stable test-nightly test-unstable

# Test and deploy targets (run tests before deployment)
.PHONY: test-and-deploy-stable
test-and-deploy-stable: test-stable deploy-stable

.PHONY: test-and-deploy-nightly
test-and-deploy-nightly: test-nightly deploy-nightly

.PHONY: test-and-deploy-unstable
test-and-deploy-unstable: test-unstable deploy-unstable

.PHONY: test-and-deploy
test-and-deploy: test all

# Clean up temporary directories
.PHONY: clean
clean:
	@echo "Cleaning up temporary directories..."
	@rm -rf $(TEMP_DIR)
	@echo "✅ Cleanup completed"

# Validate HTML files exist
.PHONY: validate
validate:
	@echo "Validating HTML files..."
	@test -f $(STABLE_HTML) || (echo "❌ $(STABLE_HTML) not found" && exit 1)
	@test -f $(NIGHTLY_HTML) || (echo "❌ $(NIGHTLY_HTML) not found" && exit 1)
	@test -f $(UNSTABLE_HTML) || (echo "❌ $(UNSTABLE_HTML) not found" && exit 1)
	@echo "✅ All HTML files found"

# Check AWS CLI availability
.PHONY: check-aws
check-aws:
	@echo "Checking AWS CLI availability..."
	@which aws > /dev/null || (echo "❌ AWS CLI not found. Please install AWS CLI." && exit 1)
	@aws --version
	@echo "✅ AWS CLI is available"

# Dry run - show what would be uploaded without actually doing it
.PHONY: dry-run-stable
dry-run-stable:
	@echo "Dry run for stable repository deployment..."
	@mkdir -p $(TEMP_DIR)/stable
	@cp $(STABLE_HTML) $(TEMP_DIR)/stable/index.html
	@aws s3 cp $(TEMP_DIR)/stable/index.html s3://$(STABLE_BUCKET)/index.html \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) \
		--dryrun
	@rm -rf $(TEMP_DIR)/stable

.PHONY: dry-run-nightly
dry-run-nightly:
	@echo "Dry run for nightly repository deployment..."
	@mkdir -p $(TEMP_DIR)/nightly
	@cp $(NIGHTLY_HTML) $(TEMP_DIR)/nightly/index.html
	@aws s3 cp $(TEMP_DIR)/nightly/index.html s3://$(NIGHTLY_BUCKET)/index.html \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) \
		--dryrun
	@rm -rf $(TEMP_DIR)/nightly

.PHONY: dry-run-unstable
dry-run-unstable:
	@echo "Dry run for unstable repository deployment..."
	@mkdir -p $(TEMP_DIR)/unstable
	@cp $(UNSTABLE_HTML) $(TEMP_DIR)/unstable/index.html
	@aws s3 cp $(TEMP_DIR)/unstable/index.html s3://$(UNSTABLE_BUCKET)/index.html \
		--region $(AWS_REGION) \
		--profile $(AWS_PROFILE) \
		--dryrun
	@rm -rf $(TEMP_DIR)/unstable

.PHONY: dry-run
dry-run: dry-run-stable dry-run-nightly dry-run-unstable

# Help target
.PHONY: help
help:
	@echo "Mina Protocol Debian Repository Deployment"
	@echo ""
	@echo "Available targets:"
	@echo ""
	@echo "Deployment:"
	@echo "  all                 - Deploy all repository pages"
	@echo "  deploy-stable       - Deploy stable repository page"
	@echo "  deploy-nightly      - Deploy nightly repository page"
	@echo "  deploy-unstable     - Deploy unstable repository page"
	@echo ""
	@echo "Testing:"
	@echo "  test                - Test all repository pages"
	@echo "  test-stable         - Test stable repository page"
	@echo "  test-nightly        - Test nightly repository page"
	@echo "  test-unstable       - Test unstable repository page"
	@echo ""
	@echo "Test and Deploy:"
	@echo "  test-and-deploy     - Test and deploy all pages"
	@echo "  test-and-deploy-stable    - Test and deploy stable page"
	@echo "  test-and-deploy-nightly   - Test and deploy nightly page"
	@echo "  test-and-deploy-unstable  - Test and deploy unstable page"
	@echo ""
	@echo "Validation:"
	@echo "  validate            - Check if all HTML files exist"
	@echo "  check-aws           - Verify AWS CLI availability"
	@echo ""
	@echo "Dry Run:"
	@echo "  dry-run             - Show what would be deployed (all)"
	@echo "  dry-run-stable      - Dry run for stable deployment"
	@echo "  dry-run-nightly     - Dry run for nightly deployment"
	@echo "  dry-run-unstable    - Dry run for unstable deployment"
	@echo ""
	@echo "Maintenance:"
	@echo "  clean               - Remove temporary directories"
	@echo "  help                - Show this help message"
	@echo ""
	@echo "Configuration:"
	@echo "  AWS_REGION:         $(AWS_REGION)"
	@echo "  AWS_PROFILE:        $(AWS_PROFILE)"
	@echo "  STABLE_BUCKET:      $(STABLE_BUCKET)"
	@echo "  NIGHTLY_BUCKET:     $(NIGHTLY_BUCKET)"
	@echo "  UNSTABLE_BUCKET:    $(UNSTABLE_BUCKET)"