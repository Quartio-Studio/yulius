import { Ac as defineStd, Ml as p, Wa as alphaOf, bo as withAlpha, io as mixOf, mo as strokeOver, xo as zigzagBands } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var bands = zigzagBands({
	count: p("count"),
	angle: p("angle"),
	balance: p("balance"),
	softness: p("softness"),
	offset: p("offset")
});
const componentDefinition = defineStd({
	name: "Chevron",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Animated chevron / zigzag stripe pattern",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#000000",
			transform: transformColor,
			description: "First color",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Second color",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		count: {
			default: 5,
			description: "Number of chevron pairs visible",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 30,
				step: 1,
				label: "Count",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Rotation angle of the chevrons",
			ui: {
				type: ["range", "map"],
				min: -180,
				max: 180,
				step: 1,
				label: "Angle",
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
				step: .01,
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
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		speed: {
			default: 0,
			description: "Animation speed",
			ui: {
				type: "range",
				min: -2,
				max: 2,
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
				step: .01,
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
	paint: withAlpha(strokeOver({
		fill: p("colorA"),
		stroke: p("colorB"),
		mask: bands,
		space: p("colorSpace")
	}), mixOf(alphaOf(p("colorA")), alphaOf(p("colorB")), bands))
});
var Chevron_default = componentDefinition;
export { componentDefinition as n, Chevron_default as t };
