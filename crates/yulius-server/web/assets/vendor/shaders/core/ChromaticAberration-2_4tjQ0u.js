import { Ac as defineStd, Ml as p, ct as chromaticFan, sn as gatherStack } from "./std-ClftfWpF.js";
import { r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "ChromaticAberration",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Separate RGB channels for a prismatic distortion effect",
	props: {
		strength: {
			default: .2,
			description: "Overall strength of the chromatic aberration effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Direction of the chromatic aberration in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		redOffset: {
			default: -1,
			description: "Red channel offset multiplier",
			ui: {
				type: ["range", "map"],
				min: -2,
				max: 2,
				step: .1,
				label: "Red Offset",
				group: "Effect"
			}
		},
		greenOffset: {
			default: 0,
			description: "Green channel offset multiplier",
			ui: {
				type: ["range", "map"],
				min: -2,
				max: 2,
				step: .1,
				label: "Green Offset",
				group: "Effect"
			}
		},
		blueOffset: {
			default: 1,
			description: "Blue channel offset multiplier",
			ui: {
				type: ["range", "map"],
				min: -2,
				max: 2,
				step: .1,
				label: "Blue Offset",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(chromaticFan({
		strength: p("strength"),
		angle: p("angle"),
		red: p("redOffset"),
		green: p("greenOffset"),
		blue: p("blueOffset")
	}), [])
});
var ChromaticAberration_default = componentDefinition;
export { componentDefinition as n, ChromaticAberration_default as t };
