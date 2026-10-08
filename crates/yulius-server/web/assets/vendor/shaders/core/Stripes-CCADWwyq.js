import { Ac as defineStd, Ml as p, mo as strokeOver, po as stripeBands } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Stripes",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Alternating colored stripes with animation",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#000000",
			transform: transformColor,
			description: "First stripe color",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Second stripe color",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		angle: {
			default: 45,
			transform: transformAngle,
			description: "Angle of stripes in degrees",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		density: {
			default: 5,
			description: "Number of stripe pairs visible",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 30,
				step: 1,
				label: "Density",
				group: "Effect"
			}
		},
		balance: {
			default: .5,
			description: "Ratio of the two colors",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Balance",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Edge softness",
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
			default: .2,
			description: "Animation speed",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		offset: {
			default: 0,
			description: "Phase offset for pattern positioning",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Offset",
				group: "Animation"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: strokeOver({
		fill: p("colorA"),
		stroke: p("colorB"),
		mask: stripeBands({
			angle: p("angle"),
			density: p("density"),
			balance: p("balance"),
			softness: p("softness"),
			offset: p("offset")
		}),
		space: p("colorSpace")
	})
});
var Stripes_default = componentDefinition;
export { componentDefinition as n, Stripes_default as t };
