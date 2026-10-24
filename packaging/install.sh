#!/bin/sh
set -eu

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
data_dir=${XDG_DATA_HOME:-"$HOME/.local/share"}
app_dir="$data_dir/lantern/app"
bin_dir="$HOME/.local/bin"
mkdir -p "$app_dir" "$bin_dir" "$data_dir/applications" "$data_dir/icons/hicolor/scalable/apps"
cp -R "$package_dir/." "$app_dir/"
for binary in lantern lantern-mcp lantern-ui; do
    chmod +x "$app_dir/$binary"
    ln -sfn "$app_dir/$binary" "$bin_dir/$binary"
done
cp "$app_dir/lantern.svg" "$data_dir/icons/hicolor/scalable/apps/dev.megalithofficial.Lantern.svg"
escaped_app_dir=$(printf '%s' "$app_dir" | sed 's/\\/\\\\\\\\/g; s/"/\\\\"/g; s/`/\\\\`/g; s/\$/\\\\$/g')
while IFS= read -r line; do
    case "$line" in
        Exec=*) printf 'Exec="%s/lantern-ui"\n' "$escaped_app_dir" ;;
        *) printf '%s\n' "$line" ;;
    esac
done < "$app_dir/dev.megalithofficial.Lantern.desktop" > "$data_dir/applications/dev.megalithofficial.Lantern.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_dir/applications"
fi
printf 'Installed Lantern to %s\nEnsure %s is on PATH.\n' "$app_dir" "$bin_dir"
