import { Ac as defineStd, Eo as pointCloudGradient, Ml as p } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "MultiPointGradient",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Five individually placed color points blended together by proximity — drag each point to shape the gradient",
	acceptsUVContext: true,
	props: {
		colorA: {
			default: "#4776E6",
			transform: transformColor,
			description: "Color of control point A",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		positionA: {
			default: {
				x: .2,
				y: .2
			},
			transform: transformPosition,
			description: "Position of control point A",
			ui: {
				type: "position",
				label: "Position A",
				group: "Position"
			}
		},
		colorB: {
			default: "#C44DFF",
			transform: transformColor,
			description: "Color of control point B",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		positionB: {
			default: {
				x: .8,
				y: .2
			},
			transform: transformPosition,
			description: "Position of control point B",
			ui: {
				type: "position",
				label: "Position B",
				group: "Position"
			}
		},
		colorC: {
			default: "#1ABC9C",
			transform: transformColor,
			description: "Color of control point C",
			ui: {
				type: "color",
				label: "Color C",
				group: "Colors"
			}
		},
		positionC: {
			default: {
				x: .2,
				y: .8
			},
			transform: transformPosition,
			description: "Position of control point C",
			ui: {
				type: "position",
				label: "Position C",
				group: "Position"
			}
		},
		colorD: {
			default: "#F8BBD9",
			transform: transformColor,
			description: "Color of control point D",
			ui: {
				type: "color",
				label: "Color D",
				group: "Colors"
			}
		},
		positionD: {
			default: {
				x: .8,
				y: .8
			},
			transform: transformPosition,
			description: "Position of control point D",
			ui: {
				type: "position",
				label: "Position D",
				group: "Position"
			}
		},
		colorE: {
			default: "#FF8C42",
			transform: transformColor,
			description: "Color of control point E",
			ui: {
				type: "color",
				label: "Color E",
				group: "Colors"
			}
		},
		positionE: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Position of control point E",
			ui: {
				type: "position",
				label: "Position E",
				group: "Position"
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
		},
		smoothness: {
			default: 2,
			description: "Controls how smoothly colors blend.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .01,
				label: "Smoothness",
				group: "Effect"
			}
		}
	},
	paint: pointCloudGradient({
		points: [
			{
				color: p("colorA"),
				position: p("positionA")
			},
			{
				color: p("colorB"),
				position: p("positionB")
			},
			{
				color: p("colorC"),
				position: p("positionC")
			},
			{
				color: p("colorD"),
				position: p("positionD")
			},
			{
				color: p("colorE"),
				position: p("positionE")
			}
		],
		smoothness: p("smoothness"),
		space: p("colorSpace")
	})
});
var MultiPointGradient_default = componentDefinition;
export { componentDefinition as n, MultiPointGradient_default as t };
