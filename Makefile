PREFIX ?= /usr/local
DESTDIR ?=
.PHONY: all check install clean
all:
	./scripts/cargo build --locked --release
check:
	./scripts/check
install: all
	install -Dm755 target/release/rcdu "$(DESTDIR)$(PREFIX)/bin/rcdu"
	install -Dm644 rcdu.1 "$(DESTDIR)$(PREFIX)/share/man/man1/rcdu.1"
clean:
	./scripts/cargo clean
