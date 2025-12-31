.PHONY: build run clean test lint release snapshot install uninstall help

APP_NAME := radio-record
VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
COMMIT := $(shell git rev-parse --short HEAD 2>/dev/null || echo "none")
DATE := $(shell date -u +"%Y-%m-%dT%H:%M:%SZ")
LDFLAGS := -s -w -X main.version=$(VERSION) -X main.commit=$(COMMIT) -X main.date=$(DATE)

## build: Build the application
build:
	go build -ldflags "$(LDFLAGS)" -o $(APP_NAME) .

## run: Build and run the application
run: build
	./$(APP_NAME)

## clean: Remove build artifacts
clean:
	rm -f $(APP_NAME)
	rm -rf dist/

## test: Run tests
test:
	go test -v ./...

## lint: Run linter
lint:
	golangci-lint run

## tidy: Tidy go modules
tidy:
	go mod tidy

## snapshot: Build snapshot with GoReleaser (no publish)
snapshot:
	goreleaser release --snapshot --clean

## release: Create a new release (requires tag)
release:
	goreleaser release --clean

## install: Install to /usr/local/bin
install: build
	cp $(APP_NAME) /usr/local/bin/$(APP_NAME)
	@echo "Installed to /usr/local/bin/$(APP_NAME)"

## uninstall: Remove from /usr/local/bin
uninstall:
	rm -f /usr/local/bin/$(APP_NAME)
	@echo "Uninstalled $(APP_NAME)"

## version: Show version info
version:
	@echo "Version: $(VERSION)"
	@echo "Commit:  $(COMMIT)"
	@echo "Date:    $(DATE)"

## help: Show this help
help:
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@sed -n 's/^##//p' $(MAKEFILE_LIST) | column -t -s ':' | sed 's/^/ /'
