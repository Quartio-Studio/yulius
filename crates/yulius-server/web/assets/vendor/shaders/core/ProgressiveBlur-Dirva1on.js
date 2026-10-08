import { Ac as defineStd, Ml as p, bn as progressiveBlur } from "./std-ClftfWpF.js";
import { c as transformPosition, r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "ProgressiveBlur",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Blur that increases progressively in one direction",
	requiresRTT: true,
	requiresChild: true,
	props: {
		intensity: {
			default: 50,
			description: "Maximum intensity of the blur effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Direction of the blur gradient (in degrees)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		center: {
			default: {
				x: 0,
				y: .5
			},
			transform: transformPosition,
			description: "Center point where blur begins",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		falloff: {
			default: 1,
			description: "Distance over which blur transitions to full strength",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Falloff",
				group: "Effect"
			}
		}
	},
	...progressiveBlur({
		intensity: p("intensity"),
		angle: p("angle"),
		center: p("center"),
		falloff: p("falloff")
	})
});
var ProgressiveBlur_default = componentDefinition;
export { componentDefinition as n, ProgressiveBlur_default as t };
