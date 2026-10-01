PREFIX ?= /usr/local
CARGO ?= cargo
CARGO_FLAGS ?= --locked
CARGO_TARGET_DIR ?= target

.PHONY: all test check install uninstall clean source rpm
all:
	$(CARGO) build --release $(CARGO_FLAGS)
test:
	$(CARGO) test $(CARGO_FLAGS)
check:
	$(CARGO) fmt --check
	$(CARGO) clippy $(CARGO_FLAGS) --all-targets -- -D warnings
install:
	install -Dm755 $(CARGO_TARGET_DIR)/release/flacker $(DESTDIR)$(PREFIX)/bin/flacker
	install -Dm644 assets/fonts/LeagueGothic-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/flacker/fonts/LeagueGothic-OFL.txt
	install -Dm644 assets/fonts/SourceSans3-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/flacker/fonts/SourceSans3-OFL.txt
	install -Dm644 packaging/io.github.gap19.FLACker.desktop $(DESTDIR)$(PREFIX)/share/applications/io.github.gap19.FLACker.desktop
	install -Dm644 assets/io.github.gap19.FLACker.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/io.github.gap19.FLACker.svg
	install -Dm644 packaging/io.github.gap19.FLACker.metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/io.github.gap19.FLACker.metainfo.xml
uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/flacker $(DESTDIR)$(PREFIX)/share/applications/io.github.gap19.FLACker.desktop $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/io.github.gap19.FLACker.svg $(DESTDIR)$(PREFIX)/share/metainfo/io.github.gap19.FLACker.metainfo.xml
	rm -f $(DESTDIR)$(PREFIX)/share/licenses/flacker/fonts/LeagueGothic-OFL.txt $(DESTDIR)$(PREFIX)/share/licenses/flacker/fonts/SourceSans3-OFL.txt
clean:
	$(CARGO) clean
source:
	./scripts/source-release.sh
rpm:
	./scripts/build-rpm.sh
