import { Ac as defineStd, Fs as radialMask, Hs as tintToward, Ml as p, Ss as isZero, ys as crosses } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Vignette",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Darkens or tints the edges of the frame, drawing attention toward the center",
	props: {
		color: {
			default: "#000000",
			transform: transformColor,
			description: "Color of the vignette at the edges",
			ui: {
				type: "color",
				label: "Color",
				group: "Colors"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center of the clear area where the vignette begins",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		radius: {
			default: .5,
			description: "Distance from center where the vignette begins to fade in",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1.5,
				step: .01,
				label: "Radius",
				group: "Effect"
			}
		},
		falloff: {
			default: .5,
			description: "Width of the transition zone from clear to full vignette",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: 1.5,
				step: .01,
				label: "Falloff",
				group: "Effect"
			}
		},
		intensity: {
			default: 1,
			description: "Strength of the vignette effect",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Effect"
			}
		}
	},
	effect: tintToward(p("color"), { amount: radialMask({
		center: p("center"),
		radius: p("radius"),
		falloff: p("falloff")
	}).times(p("intensity")) }),
	identityWhen: isZero("intensity"),
	missingChildMessage: "You must pass a child component into the Vignette shader."
});
var Vignette_default = componentDefinition;
export { componentDefinition as n, Vignette_default as t };
