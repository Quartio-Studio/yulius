import { Ac as defineStd, Ml as p, dd as LENS_MAX_SAMPLES, lt as spectralLens } from "./std-ClftfWpF.js";
import { c as transformPosition, r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "LensDistortion",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Split content into shifting chromatic layers with barrel or pincushion lens warp. Powered by Paper Shaders.",
	requiresRTT: true,
	requiresChild: true,
	props: {
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point of the lens effect — the focus zone, lens warp, circle crop and swirl all pivot around it",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		spread: {
			default: .6,
			description: "Strength of the color split; how far the color layers are pushed apart (0 is off)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Spread",
				group: "Spread"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Direction of the spread in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Spread"
			}
		},
		perspective: {
			default: .1,
			description: "Shapes the spread direction from a straight line (0) to a radial burst out from the centre (1)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Perspective",
				group: "Spread"
			}
		},
		bias: {
			default: 1,
			description: "Shifts the colors toward one end of the spread; 0 spaces them evenly",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Bias",
				group: "Spread"
			}
		},
		count: {
			default: 35,
			description: "Number of sampled color layers along the spread; higher is smoother and costlier",
			ui: {
				type: ["range", "map"],
				min: 2,
				max: 50,
				step: 1,
				label: "Count",
				group: "Spread"
			}
		},
		dispersion: {
			default: 1,
			description: "Overall amount of color dispersion; 1 gives each layer its own color from the spectrum, 0 keeps the original color",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Dispersion",
				group: "Dispersion"
			}
		},
		dispersionShift: {
			default: 0,
			description: "Balance of the dispersion between a soft circular zone at the centre and the rest; -1 keeps it in the centre only, 1 at the edges only",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Shift",
				group: "Dispersion"
			}
		},
		dispersionColor: {
			default: .6,
			description: "Rotates the dispersion colors around the hue wheel, 0 to 1 for a full turn",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Hue",
				group: "Dispersion"
			}
		},
		focusCenter: {
			default: .8,
			description: "Reduces the spread in a circular zone at the centre; 0 keeps it full to the centre",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Focus Center",
				group: "Focus"
			}
		},
		focusEdges: {
			default: 1,
			description: "Reduces the spread toward the edges; 1 restores the original image there",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Focus Edges",
				group: "Focus"
			}
		},
		lensBulge: {
			default: 0,
			description: "Radial lens warp; positive bulges out like a fisheye, negative pinches in like a pincushion",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Bulge",
				group: "Lens"
			}
		},
		lensCircle: {
			default: 0,
			description: "Squeezes pixels outside the inscribed circle inward so the outline becomes a circle",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Circle",
				group: "Lens"
			}
		},
		swirl: {
			default: .35,
			description: "Rotates the color layers around the centre by an angle growing along the spread; 0 is off",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Swirl",
				group: "Lens"
			}
		},
		noise: {
			default: 0,
			description: "Scatters the spread direction with noise; 0 is off",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Noise",
				group: "Noise"
			}
		},
		noiseFrequency: {
			default: .25,
			description: "Frequency of the direction noise; higher is finer (no effect with noise at 0)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Frequency",
				group: "Noise"
			}
		},
		noiseOffset: {
			default: 0,
			description: "Offsets the noise pattern for a different seed (no effect with noise at 0)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Seed",
				group: "Noise"
			}
		},
		grainMixer: {
			default: 0,
			description: "Scatters the spread with grain noise, breaking up the edges of the color layers",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Grain Mixer",
				group: "Grain"
			}
		},
		grainOverlay: {
			default: 0,
			description: "Post-processing black/white grain overlay",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Grain Overlay",
				group: "Grain"
			}
		}
	},
	gpu: { fragment: spectralLens({
		center: p("center"),
		spread: p("spread"),
		bias: p("bias"),
		angle: p("angle"),
		perspective: p("perspective"),
		count: p("count"),
		dispersion: p("dispersion"),
		dispersionShift: p("dispersionShift"),
		dispersionColor: p("dispersionColor"),
		focusCenter: p("focusCenter"),
		focusEdges: p("focusEdges"),
		swirl: p("swirl"),
		noise: p("noise"),
		noiseFrequency: p("noiseFrequency"),
		noiseOffset: p("noiseOffset"),
		lensBulge: p("lensBulge"),
		lensCircle: p("lensCircle"),
		grainMixer: p("grainMixer"),
		grainOverlay: p("grainOverlay")
	}) }
});
var LensDistortion_default = componentDefinition;
export { componentDefinition as n, LensDistortion_default as t };
