default: build

build:
	cargo build --release

export NAME := 'cosmic-ext-applet-cpufreq'
export APPID := 'dev.skylar.cosmic-ext-applet-cpufreq'

cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-src := cargo-target-dir / 'release' / NAME

rootdir := ''
prefix := env('HOME', '/home/skylar') / '.local'

base-dir := absolute_path(clean(rootdir / prefix))
share-dst := base-dir / 'share'

bin-dst := base-dir / 'bin' / NAME
helper-dst := base-dir / 'bin' / 'cosmic-cpufreqctl'
desktop-dst := share-dst / 'applications' / APPID + '.desktop'
polkit-dst := '/usr/share/polkit-1/actions' / APPID + '.policy'

policy-dst := base-dir / 'bin' / APPID + '.policy'

install:
	install -Dm0755 {{ bin-src }} {{ bin-dst }}
	install -Dm0755 data/cpufreqctl {{ helper-dst }}
	install -Dm0644 data/{{ APPID }}.policy {{ policy-dst }}
	install -Dm0644 data/{{ APPID }}.desktop {{ desktop-dst }}

install-polkit:
	install -Dm0644 data/{{ APPID }}.policy {{ polkit-dst }}

uninstall:
	rm -f {{ bin-dst }}
	rm -f {{ helper-dst }}
	rm -f {{ desktop-dst }}
	rm -f {{ polkit-dst }}
