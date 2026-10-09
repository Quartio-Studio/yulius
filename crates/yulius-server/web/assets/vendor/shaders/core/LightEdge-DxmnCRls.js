import { Ac as defineStd, Ct as edgeGlowBand, Et as orbitSpotsAccum, Hd as call, Ic as animatedTime, Kd as floatE, Ru as shapeEffectBoundingBoxDeclaration, St as edgeGlowAccumZero, Tt as heartbeatPulse, Vd as asLocal, bt as colorAtIndex, hi as shapedSurface, ou as colorStopsPropConfig, wt as edgeGlowCompose, xt as edgeGlowAccumColorFor } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var DEFAULT_STOPS = [
	{
		color: "#7b2ff7",
		position: 0
	},
	{
		color: "#00e0ff",
		position: .5
	},
	{
		color: "#ff3d81",
		position: 1
	}
];
const componentDefinition = defineStd({
	name: "LightEdge",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Glowing, pulsing light racing around the edge of any 2D, SVG, or 3D shape. Powered by Paper Shaders.",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		chord: "firstLobe",
		stencil: "centre",
		surface: (frame, params) => {
			const { uniforms, propValues } = params;
			const t = animatedTime(params);
			const beat = asLocal(call(heartbeatPulse, "heartbeatPulse", [t]), "beat");
			const bAndA = call(edgeGlowBand, "edgeGlowBand", [
				frame.surf0.member("r"),
				frame.sdfUV,
				params.ctx.viewportSize,
				uniforms.scale,
				uniforms.thickness,
				uniforms.softness,
				uniforms.smokeSize,
				uniforms.smoke,
				t,
				uniforms.pulse,
				beat
			]);
			const border = bAndA.member("x");
			const angle01 = bAndA.member("y");
			const stopCount = propValues.stopCount ?? 0;
			const colorCount = stopCount > 1 ? Math.min(stopCount, 8) : 2;
			const accumColorFn = edgeGlowAccumColorFor(propValues.colorSpace ?? 0);
			const accumFor = (idx) => call(orbitSpotsAccum, "orbitSpotsAccum", [
				angle01,
				border,
				t,
				beat,
				uniforms.spots,
				uniforms.spotSize,
				uniforms.pulse,
				uniforms.intensity,
				uniforms.softness,
				uniforms.seed,
				floatE(idx)
			]);
			const colorFor = (idx) => stopCount > 1 ? call(colorAtIndex, "colorAtIndex", [uniforms.colorsArray, floatE(idx)]) : idx === 0 ? uniforms.colorA : uniforms.colorB;
			let state = call(edgeGlowAccumZero, "edgeGlowAccumZero", []);
			for (let i = 0; i < colorCount; i++) state = call(accumColorFn, "edgeGlowAccumColor", [
				state,
				colorFor(i),
				accumFor(i)
			]);
			return call(edgeGlowCompose, "edgeGlowCompose", [state, uniforms.bloom]);
		}
	}),
	props: {
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the shape relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the shape (1 = default size)",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .01,
				label: "Scale",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		colorA: {
			default: "#7b2ff7",
			transform: transformColor,
			description: "First border light color (used when no gradient stops are set)",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#00e0ff",
			transform: transformColor,
			description: "Second border light color (used when no gradient stops are set)",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: {
			...colorStopsPropConfig(),
			default: DEFAULT_STOPS
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space overlapping lights blend in",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		thickness: {
			default: .25,
			description: "Width of the border band",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: 1,
				step: .01,
				label: "Thickness",
				group: "Border"
			}
		},
		softness: {
			default: .5,
			description: "Edge softness — 0 = crisp band, 1 = wide smooth gradient",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Border"
			}
		},
		intensity: {
			default: .6,
			description: "Brightness of the individual light spots",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Light"
			}
		},
		bloom: {
			default: .3,
			description: "How additively overlapping lights pile up — high values overdrive to white",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Bloom",
				group: "Light"
			}
		},
		spots: {
			default: 3,
			description: "Number of light spots orbiting per color",
			ui: {
				type: "range",
				min: 1,
				max: 5,
				step: 1,
				label: "Spots",
				group: "Light"
			}
		},
		spotSize: {
			default: .5,
			description: "Angular size of each light spot",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Spot Size",
				group: "Light"
			}
		},
		pulse: {
			default: 0,
			description: "Heartbeat pulsing — synchronizes the lights to a double-beat rhythm",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Pulse",
				group: "Animation"
			}
		},
		smoke: {
			default: .3,
			description: "Noisy luminous smoke drifting along the border",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Smoke",
				group: "Smoke"
			}
		},
		smokeSize: {
			default: .5,
			description: "Scale of the smoke billows",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Smoke Size",
				group: "Smoke"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed. 0 pauses.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed — re-rolls spot speeds, directions and phases",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Animation"
			}
		},
		shape: {
			default: DEFAULT_SHAPE_CONFIG,
			description: "Serialized shape configuration (JSON)",
			ui: {
				type: "shape",
				label: "Shape",
				group: "Shape"
			}
		},
		shapeSdfUrl: {
			default: "",
			compileTime: true,
			description: "URL to a pre-generated SDF .bin file"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type"
		}
	}
});
var LightEdge_default = componentDefinition;
export { componentDefinition as n, LightEdge_default as t };
