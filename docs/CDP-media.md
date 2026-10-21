# Media preference emulation

`Emulation.setEmulatedMedia` accepts `prefers-color-scheme` and
`prefers-reduced-motion` in its `features` array:

```json
{"features":[{"name":"prefers-color-scheme","value":"dark"},{"name":"prefers-reduced-motion","value":"reduce"}]}
```

Color-scheme values are `dark`, `light`, `no-preference`, and the empty string.
The last two restore the default light preference; the obsolete CSS query
`(prefers-color-scheme: no-preference)` does not match. Reduced-motion values
are `reduce`, `no-preference`, and the empty string.
`forced-colors` accepts `none` and the empty string: both use the engine's
normal palette, and `(forced-colors: none)` matches. `active` is unsupported
and returns an error; accepting the default does not emulate a forced palette.
`prefers-contrast` accepts `no-preference` and the empty string, retaining the
normal contrast palette. `(prefers-contrast: no-preference)` matches; `more`,
`less`, and `custom` emulation remain unsupported and return an error.

A supplied array replaces both supported preferences. `features: []` clears
both overrides. Omitting `features` also clears them. Invalid supported values
or malformed feature entries return an error before changing page state.

Overrides persist across navigation and apply to existing and newly created
child documents. `matchMedia()` results and change events work with and without
rendering. Render builds also update CSS media selection, live computed styles,
geometry, screenshots (including ad-hoc capture viewports), and the preference
used during PDF print layout.
The Rust `Page::set_media_preferences(reduce, dark)` method applies the same
pair of preferences directly.

Nonempty `media` types and unknown feature names are unsupported and return a
protocol error. Validation is atomic: an unsupported type, feature or value
does not partially apply other preferences from the same request. An empty
`media` string is accepted without selecting another media type.

`Input.dispatchTouchEvent` and `Emulation.setTouchEmulationEnabled` return
explicit unsupported errors. They do not synthesize mouse, pointer or touch
events, or change `maxTouchPoints` and pointer/hover media capabilities.

The CSS `color-scheme` property
and automatic darkening are separate from this media-query override. Dynamically
loaded CSS imports still use the loader's existing fetch-time media filter;
a preference change does not refetch an import previously skipped by it.

## Native disclosure and range input

Script `click()` and CDP primary-button activation share the protected default
activation path. Activating a details element's first direct `summary` toggles
`open`; a cancelled click, later summary, or independently interactive child
does not perform that toggle. Programmatic `open` changes also queue `toggle`
notifications, coalescing same-task changes while preserving the first old
state and final new state. Script clicks remain untrusted; CDP input is trusted.

Horizontal `input[type=range]` controls share their numeric sanitization across
`value`, `valueAsNumber`, content attributes, native pointer/drag/keyboard input
and rendering. The default value is `50`. Supported keyboard updates include
Home/End and arrow keys, including RTL direction; dragging sends progressive
trusted `input` events and commits `change` on release even outside the control.
Cancelled input, secondary-button input and disabled controls do not change the
value. Programmatic assignments do not manufacture user input/change events.

Render builds draw a native track and thumb using the current sanitized value.
`appearance:none` suppresses that theme without disabling input semantics.
Vertical sliders, author slider pseudo-elements and touch gestures are not
implemented by this horizontal-control support.

`Emulation.setTouchEmulationEnabled` accepts `enabled: false`, the normal
mouse-only client default. `enabled: true` and `Input.dispatchTouchEvent`
remain explicitly unsupported; disabling an absent feature does not advertise
touch input support.
