#!/bin/sh
# Instala Huella: descarga el binario de la última release, comprueba su
# checksum, lo deja en ~/.local/bin y lanza `huella setup`.
#
#   curl -fsSL https://raw.githubusercontent.com/juligarc10/huella/main/install.sh | sh
#
# Variables opcionales:
#   HUELLA_VERSION       versión a instalar (p. ej. v0.1.0; por defecto la última)
#   HUELLA_INSTALL_DIR   carpeta del binario (por defecto ~/.local/bin)
#   HUELLA_NO_SETUP=1    no lanza `huella setup` al terminar
#   HUELLA_DOWNLOAD_URL  base de descarga alternativa (un espejo); por defecto
#                        https://github.com/juligarc10/huella/releases/download

set -eu

REPO="https://github.com/juligarc10/huella"

say() { printf '%s\n' "$*"; }
fail() {
	printf 'huella: %s\n' "$*" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || fail "hace falta '$1' y no está instalado"
}

detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$os" in
	Linux) os_part="unknown-linux-gnu" ;;
	Darwin) os_part="apple-darwin" ;;
	*) fail "sistema no soportado: $os (en Windows usa install.ps1)" ;;
	esac
	case "$arch" in
	x86_64 | amd64) arch_part="x86_64" ;;
	arm64 | aarch64) arch_part="aarch64" ;;
	*) fail "arquitectura no soportada: $arch" ;;
	esac
	if [ "$os_part" = "unknown-linux-gnu" ] && [ "$arch_part" = "aarch64" ]; then
		fail "aún no hay binario para Linux ARM; instala con: cargo install --git $REPO huella-cli"
	fi
	printf '%s-%s' "$arch_part" "$os_part"
}

# Última versión publicada, por la redirección de /releases/latest (sin la API,
# que limita las peticiones anónimas).
latest_version() {
	url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPO/releases/latest") ||
		fail "no se pudo consultar la última versión en $REPO/releases"
	version=${url##*/}
	case "$version" in
	v[0-9]*) printf '%s' "$version" ;;
	*) fail "no hay ninguna release publicada todavía" ;;
	esac
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d ' ' -f 1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d ' ' -f 1
	else
		fail "hace falta sha256sum o shasum para comprobar la descarga"
	fi
}

# Borra la carpeta temporal conservando el código de salida: un fallo no
# puede acabar en 0.
cleanup() {
	status=$?
	rm -rf "$work"
	exit "$status"
}

main() {
	need curl
	need tar
	need uname

	target=$(detect_target)
	version=${HUELLA_VERSION:-$(latest_version)}
	base=${HUELLA_DOWNLOAD_URL:-"$REPO/releases/download"}
	install_dir=${HUELLA_INSTALL_DIR:-"$HOME/.local/bin"}
	archive="huella-$version-$target.tar.gz"

	work=$(mktemp -d)
	trap cleanup EXIT
	trap 'exit 130' INT TERM

	say "Descargando Huella ${version} para ${target}…"
	curl -fsSL -o "$work/$archive" "$base/$version/$archive" ||
		fail "no se pudo descargar $base/$version/$archive"
	curl -fsSL -o "$work/SHA256SUMS" "$base/$version/SHA256SUMS" ||
		fail "la release $version no publica SHA256SUMS: no se instala sin comprobarla"

	expected=$(awk -v name="$archive" '$2 == name || $2 == "*" name { print $1 }' "$work/SHA256SUMS")
	[ -n "$expected" ] || fail "SHA256SUMS no incluye $archive"
	actual=$(sha256_of "$work/$archive")
	[ "$expected" = "$actual" ] || fail "el checksum de $archive no coincide: la descarga está corrupta o la han cambiado"
	say "Checksum comprobado."

	tar -xzf "$work/$archive" -C "$work"
	mkdir -p "$install_dir"
	# Copia y renombra: así no queda un binario a medias si algo falla.
	cp "$work/huella-$version-$target/huella" "$install_dir/huella.new"
	chmod 755 "$install_dir/huella.new"
	mv "$install_dir/huella.new" "$install_dir/huella"
	say "Instalado en $install_dir/huella"

	case ":$PATH:" in
	*":$install_dir:"*) ;;
	*)
		say ""
		say "$install_dir no está en tu PATH. Añade a tu perfil del shell:"
		say "  export PATH=\"$install_dir:\$PATH\""
		;;
	esac

	if [ "${HUELLA_NO_SETUP:-}" = "1" ]; then
		say ""
		say "Para terminar de configurarlo: $install_dir/huella setup"
	elif [ -r /dev/tty ] && [ -w /dev/tty ]; then
		# Con `curl … | sh` la entrada estándar es este script: las preguntas
		# de huella setup van a la terminal.
		say ""
		"$install_dir/huella" setup </dev/tty
	else
		say ""
		say "Sin terminal para preguntar. Termina con: $install_dir/huella setup"
	fi
}

main "$@"
