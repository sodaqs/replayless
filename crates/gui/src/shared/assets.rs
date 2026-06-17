//! Window asset source: the app's own brand icon plus gpui-component's bundled
//! icon set (the `IconName` status badges and the title-bar window controls).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

/// The app's own embedded files. Only the small brand icon is bundled for the
/// UI — the larger sizes in `assets/` exist for the OS/installer, not drawn here.
#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "logo.svg"]
struct AppAssets;

/// Asset source registered on the `Application`. gpui resolves every `img()` /
/// `svg()` resource path through here: app files win, otherwise we fall back to
/// gpui-component's Lucide icon set (`icons/*.svg`), which backs both `IconName`
/// badges and the `TitleBar` minimize / maximize / restore / close buttons.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = AppAssets::get(path) {
            return Ok(Some(file.data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_component_assets::Assets.list(path)?;
        paths.extend(
            AppAssets::iter()
                .filter(|p| p.starts_with(path))
                .map(SharedString::from),
        );
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_app_brand_icon() {
        assert!(Assets.load("logo.svg").unwrap().is_some());
    }

    #[test]
    fn falls_back_to_component_window_control_icons() {
        // These back the TitleBar minimize / maximize / restore / close buttons.
        for icon in [
            "icons/window-minimize.svg",
            "icons/window-maximize.svg",
            "icons/window-restore.svg",
            "icons/window-close.svg",
        ] {
            assert!(Assets.load(icon).unwrap().is_some(), "missing {icon}");
        }
    }

    #[test]
    fn unknown_asset_is_not_found() {
        assert!(Assets.load("icons/definitely-not-real.svg").is_err());
    }
}
