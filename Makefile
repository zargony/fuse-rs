RUSTC ?= rustc
RUSTFLAGS ?= -O --cfg ndebug

LIBFUSE := $(patsubst %,build/%,$(shell $(RUSTC) --crate-file-name src/lib.rs))

all: $(LIBFUSE)

check: build/libfuse_test
	build/libfuse_test

clean:
	rm -rf build

.PHONY: all check clean

$(LIBFUSE): src/lib.rs
	mkdir -p build
	$(RUSTC) $(RUSTFLAGS) --dep-info build/libfuse.d --out-dir $(dir $@) $<

-include build/libfuse.d

build/libfuse_test: src/lib.rs
	mkdir -p build
	$(RUSTC) $(RUSTFLAGS) --dep-info build/libfuse_test.d --test -o $@ $<

-include build/libfuse_test.d

EXAMPLE_SRCS := $(wildcard examples/*.rs)
EXAMPLE_BINS := $(patsubst examples/%.rs,build/%,$(EXAMPLE_SRCS))

examples: $(EXAMPLE_BINS)

.PHONY: examples

$(EXAMPLE_BINS): build/%: examples/%.rs $(LIBFUSE)
	$(RUSTC) $(RUSTFLAGS) -L build -Z prefer-dynamic -o $@ $<

INTEGRATION_TEST_BIN = build/libfuse_test_integration
INTEGRATION_TEST_HELPER_DIR = build/integration-test-helpers
INTEGRATION_TEST_HELPER_BINS = $(addprefix $(INTEGRATION_TEST_HELPER_DIR)/,$(basename $(notdir $(wildcard src/integration-test-helpers/*.rs))))

$(INTEGRATION_TEST_BIN): src/test-integration.rs $(LIBFUSE)
	mkdir -p $(dir $@)
	$(RUSTC) $(RUSTFLAGS) -L build -Z prefer-dynamic --dep-info $@.d --test -o $@ $<

$(INTEGRATION_TEST_HELPER_DIR)/%: src/integration-test-helpers/%.rs $(LIBFUSE)
	mkdir -p $(dir $@)
	$(RUSTC) $(RUSTFLAGS) -L build -Z prefer-dynamic -o $@ $<

-include build/$(INTEGRATION_TEST_BIN).d $(addsuffix .d,$(INTEGRATION_TEST_HELPER_BINS))

test-integration: $(INTEGRATION_TEST_BIN) $(INTEGRATION_TEST_HELPER_BINS)

check-integration: test-integration examples
	env RUST_THREADS=1 $(INTEGRATION_TEST_BIN)
