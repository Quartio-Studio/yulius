import { Ac as defineStd, Co as beamPreconvertedFields, Ml as p, So as beam } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Beam",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "A beam of light from one point to another.",
	acceptsUVContext: true,
	extraFields: beamPreconvertedFields,
	props: {
		startPosition: {
			default: {
				x: .2,
				y: .5
			},
			transform: transformPosition,
			description: "Starting point of the beam",
			ui: {
				type: "position",
				label: "Start Position",
				group: "Position"
			}
		},
		endPosition: {
			default: {
				x: .8,
				y: .5
			},
			transform: transformPosition,
			description: "Ending point of the beam",
			ui: {
				type: "position",
				label: "End Position",
				group: "Position"
			}
		},
		startThickness: {
			default: .2,
			description: "Thickness at the start of the beam",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "Start Thickness",
				group: "Effect"
			}
		},
		endThickness: {
			default: .2,
			description: "Thickness at the end of the beam",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .1,
				label: "End Thickness",
				group: "Effect"
			}
		},
		startSoftness: {
			default: .5,
			description: "Edge softness at the start of the beam",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 50,
				step: .1,
				label: "Start Softness",
				group: "Effect"
			}
		},
		endSoftness: {
			default: .5,
			description: "Edge softness at the end of the beam",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .1,
				label: "End Softness",
				group: "Effect"
			}
		},
		insideColor: {
			default: "#FF0000",
			transform: transformColor,
			description: "Color at the center of the beam",
			ui: {
				type: "color",
				label: "Inside Color",
				group: "Colors"
			}
		},
		outsideColor: {
			default: "#0000FF",
			transform: transformColor,
			description: "Color at the edges of the beam",
			ui: {
				type: "color",
				label: "Outside Color",
				group: "Colors"
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
	paint: beam({
		from: p("startPosition"),
		to: p("endPosition"),
		thickness: {
			start: p("startThickness"),
			end: p("endThickness")
		},
		softness: {
			start: p("startSoftness"),
			end: p("endSoftness")
		},
		colors: {
			inside: p("insideColor"),
			outside: p("outsideColor")
		},
		space: p("colorSpace")
	})
});
var Beam_default = componentDefinition;
export { componentDefinition as n, Beam_default as t };
