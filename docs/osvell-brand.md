# Osvell icon

The approved identity is a blue cinema reel with three asymmetric apertures and
a small spindle opening. The mark is flat, with transparent negative space and
no surrounding tile, outlines, gradients, shadows, or bevels.

- Canonical vector: `studio/src-tauri/icons/osvell-master.svg`
- Color: `#4389FF`
- Raster master: `studio/src-tauri/icons/osvell-master.png`
- Browser favicon: `studio/public/osvell.png`

From `studio/`, regenerate the platform icons with:

```powershell
npm exec -- tauri icon src-tauri/icons/osvell-master.svg --output src-tauri/icons
Copy-Item src-tauri/icons/128x128.png public/osvell.png
```

The Tauri bundle and Windows installer already consume this icon directory.
Preserve the app identifier and executable name when publishing the new artwork;
existing pinned shortcuts depend on those compatibility contracts.

The concept was created with the built-in image generation tool, then traced to
the vector master to remove extraction artifacts and keep small exports clean.
The final production brief was: preserve the approved reel silhouette, its three
asymmetric cutouts and spindle opening; use one blue fill with transparent
background and cutouts, without text, a tile, or dimensional effects.
