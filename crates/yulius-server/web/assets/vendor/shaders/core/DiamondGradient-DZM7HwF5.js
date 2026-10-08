import { Ac as defineStd, Bo as rings, Ml as p, ko as dist, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "DiamondGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Diamond-shaped gradient radiating from a center point using Manhattan distance",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#4ffb4a",
			transform: transformColor,
			description: "Color at the center of the diamond",
			ui: {
				type: "color",
				label: "Color A (Center)",
				group: "Colors"
			}
		},
		colorB: {
			default: "#4f1238",
			transform: transformColor,
			description: "Color at the outer edges of the diamond",
			ui: {
				type: "color",
				label: "Color B (Edge)",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center point of the diamond",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		size: {
			default: .7,
			description: "Extent of the gradient — controls how far Color A reaches before transitioning to Color B",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: 2,
				step: .01,
				label: "Size",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation in degrees — tilts the diamond into a rhombus",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		},
		repeat: {
			default: 1,
			description: "Number of times the gradient repeats outward. Values above 1 create concentric diamond or square bands.",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 16,
				step: .5,
				label: "Repeat",
				group: "Effect"
			}
		},
		roundness: {
			default: 0,
			description: "Morphs from a sharp diamond (0) to a square (1)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Roundness",
				group: "Effect"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: rampOver(rings(dist.diamond({
		center: p("center"),
		size: p("size"),
		rotation: p("rotation"),
		roundness: p("roundness")
	}), p("repeat")), stops(p("colorSpace")))
});
var DiamondGradient_default = componentDefinition;
export { componentDefinition as n, DiamondGradient_default as t };
