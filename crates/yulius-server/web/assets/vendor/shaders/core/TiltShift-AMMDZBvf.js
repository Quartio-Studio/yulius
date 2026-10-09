import { Ac as defineStd, Ml as p, Pn as tiltShift } from "./std-ClftfWpF.js";
import { c as transformPosition, r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "TiltShift",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Selective focus blur mimicking tilt-shift photography",
	requiresRTT: true,
	requiresChild: true,
	props: {
		intensity: {
			default: 50,
			description: "Maximum blur intensity at edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Effect"
			}
		},
		width: {
			default: .3,
			description: "Width of the sharp focus area",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Width",
				group: "Effect"
			}
		},
		falloff: {
			default: .3,
			description: "Distance over which blur transitions to full strength",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Falloff",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			transform: transformAngle,
			description: "Rotation angle of the focus line (in degrees)",
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
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center point of the focus line",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		}
	},
	...tiltShift({
		intensity: p("intensity"),
		width: p("width"),
		falloff: p("falloff"),
		angle: p("angle"),
		center: p("center")
	})
});
var TiltShift_default = componentDefinition;
export { componentDefinition as n, TiltShift_default as t };
