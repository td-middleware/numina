.PHONY: all build test clean install lint fmt check

all: build

build:
	cargo build --release

build-dev:
	cargo build

test:
	cargo test --all

clean:
	cargo clean

install: build
	cargo install --path .

lint:
	cargo clippy --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings

run:
	cargo run --release --

run-dev:
	cargo run --

# Development helpers
watch:
	cargo watch -x build -x test

# Documentation
doc:
	cargo doc --no-deps --open

# Examples
example-chat:
	cargo run --release -- chat "Hello, Numina!"

example-plan:
	cargo run --release -- plan create my-plan "Example plan"
	cargo run --release -- plan execute my-plan

example-agent:
	cargo run --release -- agent create tester --role "Test Agent"
	cargo run --release -- agent list
