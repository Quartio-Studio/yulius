import { Ac as defineStd, Io as pair, Ml as p, ko as dist, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Spiral",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Rotating spiral pattern with animated movement",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#000000",
			transform: transformColor,
			description: "Background color",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Spiral stroke color",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		strokeWidth: {
			default: .5,
			description: "Thickness of spiral stroke",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "Stroke Width",
				group: "Effect"
			}
		},
		strokeFalloff: {
			default: 0,
			description: "Stroke losing width further from center",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Stroke Falloff",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Color transition sharpness (0 = hard edge, 1 = smooth fade)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Softness",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed (negative values reverse direction)",
			ui: {
				type: "range",
				min: -3,
				max: 3,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point of the spiral",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		scale: {
			default: 1,
			description: "Scale factor for spiral bands (higher = more bands, lower = fewer bands)",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 5,
				step: .1,
				label: "Scale",
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
	paint: rampOver(dist.spiral({
		center: p("center"),
		scale: p("scale"),
		width: p("strokeWidth"),
		falloff: p("strokeFalloff"),
		softness: p("softness")
	}), pair(p("colorA"), p("colorB"), p("colorSpace")))
});
var Spiral_default = componentDefinition;
export { componentDefinition as n, Spiral_default as t };
