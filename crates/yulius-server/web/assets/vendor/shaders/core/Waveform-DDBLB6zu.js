import { $s as fract, Ac as defineStd, Al as uniformOf, Cc as sqrt, Gs as clamp, Ic as animatedTime, Ml as p, Ms as crispFill, Ns as crispStroke, Oc as vec4, Qs as floor, Tc as sub, Us as abs, Ws as add, bc as smoothstep, cc as max, dc as mul, dr as surfaceOf, ec as gaussBell, fc as neg, fs as roundedRect, oc as local, ou as colorStopsPropConfig, qo as stops, qs as div, rs as circle, sa as waves, uc as mix, ur as segmentFrame, wc as step } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var SPECTRUM = [
	{
		waves: [{
			x: 5.1,
			t: 1.3
		}],
		weight: .28
	},
	{
		waves: [{
			x: 9.7,
			t: -1.9
		}, {
			x: 1.7,
			t: .6
		}],
		weight: .24
	},
	{
		waves: [{
			x: 17.3,
			t: 2.7
		}],
		weight: .18
	},
	{
		waves: [{
			x: 31.9,
			t: -3.4
		}, {
			x: 3.3,
			t: -.9
		}],
		weight: .14
	},
	{
		waves: [{
			x: 67.7,
			t: 4.1
		}],
		weight: .09
	},
	{
		waves: [{
			x: 131.3,
			t: -5.3
		}, {
			x: 7.9,
			t: 1.1
		}],
		weight: .07
	}
];
var EDGE_LEVEL = .4;
var ENVELOPE_FALLOFF = 4;
var LEVEL_FLOOR = .45;
var LEVEL_SWING = .6;
var SLOPE_EPS = .002;
var STYLE_MODES = {
	bars: 0,
	wave: 1,
	line: 2,
	dots: 3
};
var ALIGN_MODES = {
	mirrored: 0,
	top: 1,
	bottom: 2
};
var styleOf = (raw) => {
	if (typeof raw === "number") return Object.keys(STYLE_MODES)[raw] ?? "bars";
	return raw === "wave" || raw === "line" || raw === "dots" ? raw : "bars";
};
var alignOf = (raw) => {
	if (typeof raw === "number") return Object.keys(ALIGN_MODES)[raw] ?? "mirrored";
	return raw === "top" || raw === "bottom" ? raw : "mirrored";
};
const componentDefinition = defineStd({
	name: "Waveform",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Audio-visualizer waveform — equalizer bars, a filled wave, an oscilloscope line, or a dot matrix — driven by a simulated signal whose amplitude you can map to your own audio level",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		style: {
			default: "bars",
			transform: (value) => STYLE_MODES[value] ?? 0,
			compileTime: true,
			description: "Visualizer style: equalizer bars, a filled wave envelope, an oscilloscope line, or a dot matrix",
			ui: {
				type: "select",
				options: [
					{
						label: "Bars",
						value: "bars"
					},
					{
						label: "Wave",
						value: "wave"
					},
					{
						label: "Line",
						value: "line"
					},
					{
						label: "Dots",
						value: "dots"
					}
				],
				label: "Style",
				group: "Style"
			}
		},
		colorA: {
			default: "#3b5bff",
			transform: transformColor,
			description: "Color at the start",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ff3bd4",
			transform: transformColor,
			description: "Color at the end",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for the start→end gradient",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		from: {
			default: {
				x: 0,
				y: .5
			},
			transform: transformPosition,
			description: "Where the waveform starts (Color A end)",
			ui: {
				type: "position",
				label: "Start",
				group: "Position",
				units: ["%", "px"]
			}
		},
		to: {
			default: {
				x: 1,
				y: .5
			},
			transform: transformPosition,
			description: "Where the waveform ends (Color B end)",
			ui: {
				type: "position",
				label: "End",
				group: "Position",
				units: ["%", "px"]
			}
		},
		amplitude: {
			default: 1,
			description: "Overall signal level (0 = silence). Map this to your own audio level to make the waveform react to real sound.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Amplitude",
				group: "Signal"
			}
		},
		frequency: {
			default: 1,
			description: "How busy the signal is across the width (higher = more peaks)",
			ui: {
				type: ["range", "map"],
				min: .2,
				max: 3,
				step: .01,
				label: "Frequency",
				group: "Signal"
			}
		},
		height: {
			default: .6,
			description: "Maximum vertical extent of the waveform as a fraction of the canvas height",
			ui: {
				type: ["range", "map"],
				min: .05,
				max: 1,
				step: .01,
				label: "Height",
				group: "Effect"
			}
		},
		align: {
			default: "mirrored",
			transform: (value) => ALIGN_MODES[value] ?? 0,
			compileTime: true,
			description: "Which side of the start→end baseline the waveform grows on: mirrored around it, above it (top), or below it (bottom)",
			ui: {
				type: "select",
				options: [
					{
						label: "Mirrored",
						value: "mirrored"
					},
					{
						label: "Top",
						value: "top"
					},
					{
						label: "Bottom",
						value: "bottom"
					}
				],
				label: "Align",
				group: "Effect"
			}
		},
		count: {
			default: 32,
			description: "Number of bars (or dot columns) across the width",
			ui: {
				type: ["range", "map"],
				min: 4,
				max: 128,
				step: 1,
				label: "Count",
				group: "Effect",
				condition: { style: ["bars", "dots"] }
			}
		},
		barWidth: {
			default: .6,
			description: "Bar thickness as a fraction of its column (1 = bars touch)",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 1,
				step: .01,
				label: "Bar Width",
				group: "Effect",
				condition: { style: "bars" }
			}
		},
		rounding: {
			default: 1,
			description: "Rounds the bar ends (0 = square, 1 = fully rounded caps)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Rounding",
				group: "Effect",
				condition: { style: "bars" }
			}
		},
		dotSize: {
			default: .7,
			description: "Dot diameter as a fraction of its cell",
			ui: {
				type: ["range", "map"],
				min: .2,
				max: 1,
				step: .01,
				label: "Dot Size",
				group: "Effect",
				condition: { style: "dots" }
			}
		},
		lineWidth: {
			default: .006,
			description: "Stroke thickness of the line. A value of one (1) matches the canvas height.",
			ui: {
				type: ["range", "map"],
				min: .001,
				max: .05,
				step: .001,
				label: "Line Width",
				group: "Effect",
				dimensional: "canvas-height",
				condition: { style: "line" }
			}
		},
		softness: {
			default: 0,
			description: "Feathers the wave's edge into a glow",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect",
				condition: { style: "wave" }
			}
		},
		speed: {
			default: 1,
			description: "Animation speed of the simulated signal",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 1,
			description: "Adjusts the starting state, useful for variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Animation"
			}
		}
	},
	paint: (params) => {
		const style = styleOf(params.propValues.style);
		const align = alignOf(params.propValues.align);
		const { viewport } = surfaceOf(params);
		const u = (name) => uniformOf(p(name), params);
		const { along, across, length } = segmentFrame({
			from: p("from"),
			to: p("to")
		})(params, surfaceOf(params));
		const x = local(div(along, length), "wfX");
		const px = local(div(1, viewport.member("y")), "wfPx");
		const t = local(animatedTime(params, "seed"), "wfTime");
		const amplitude = u("amplitude");
		const frequency = u("frequency");
		const height = u("height");
		const rawAt = (xs) => waves({
			x: mul(xs, frequency),
			t
		}, SPECTRUM);
		const envelopeAt = (xs) => mix(EDGE_LEVEL, 1, gaussBell(sub(xs, .5), ENVELOPE_FALLOFF));
		const signalAt = (xs) => clamp(mul(mul(rawAt(xs), envelopeAt(xs)), amplitude), -1, 1);
		const levelAt = (xs) => clamp(mul(mul(add(LEVEL_FLOOR, mul(rawAt(xs), LEVEL_SWING)), envelopeAt(xs)), amplitude), 0, 1);
		const mirrored = align === "mirrored";
		const dy = local(align === "top" ? neg(across) : across, "wfDy");
		const rise = mirrored ? abs(dy) : dy;
		const extentOf = (level) => mul(mul(level, height), mirrored ? .5 : 1);
		const bandDistance = (extent) => mirrored ? sub(rise, extent) : max(sub(rise, extent), neg(rise));
		const count = u("count");
		const cell = local(div(length, count), "wfCell");
		const xCells = local(mul(x, count), "wfXCells");
		const columnX = local(div(add(floor(xCells), .5), count), "wfColX");
		const acrossColumn = local(mul(sub(fract(xCells), .5), cell), "wfColDx");
		let coverage;
		if (style === "bars") {
			const halfW = local(mul(mul(u("barWidth"), cell), .5), "wfHalfW");
			const extent = local(max(extentOf(levelAt(columnX)), mirrored ? halfW : mul(halfW, 2)), "wfExtent");
			const halfH = mirrored ? extent : mul(extent, .5);
			coverage = crispFill({
				distance: roundedRect(acrossColumn, mirrored ? dy : sub(dy, halfH), halfW, halfH, mul(u("rounding"), halfW)),
				footprint: px
			});
		} else if (style === "dots") {
			const rows = local(div(rise, cell), "wfRows");
			const rowCentre = local(mul(add(floor(rows), .5), cell), "wfRowY");
			const extent = local(extentOf(levelAt(columnX)), "wfExtent");
			const lit = mul(sub(1, smoothstep(sub(extent, mul(cell, .5)), add(extent, mul(cell, .5)), rowCentre)), mirrored ? 1 : step(0, rise));
			const radius = mul(mul(u("dotSize"), cell), .5);
			coverage = mul(crispFill({
				distance: circle(acrossColumn, mul(sub(fract(rows), .5), cell), radius),
				footprint: px
			}), lit);
		} else if (style === "line") {
			const curveAt = (xs) => mirrored ? extentOf(signalAt(xs)) : extentOf(levelAt(xs));
			const curve = local(curveAt(x), "wfCurve");
			const slope = local(div(sub(curveAt(add(x, SLOPE_EPS)), curveAt(sub(x, SLOPE_EPS))), mul(2 * SLOPE_EPS, length)), "wfSlope");
			coverage = crispStroke({
				distance: div(sub(dy, curve), sqrt(add(1, mul(slope, slope)))),
				width: u("lineWidth"),
				footprint: px
			});
		} else coverage = crispFill({
			distance: bandDistance(local(extentOf(levelAt(x)), "wfExtent")),
			footprint: add(px, mul(u("softness"), .2))
		});
		const inSpan = crispFill({
			distance: max(neg(along), sub(along, length)),
			footprint: px
		});
		const color = local(stops(p("colorSpace"))(x, params), "wfColor");
		return vec4(color.member("rgb"), mul(mul(color.member("a"), coverage), inSpan));
	}
});
var Waveform_default = componentDefinition;
export { componentDefinition as n, Waveform_default as t };
