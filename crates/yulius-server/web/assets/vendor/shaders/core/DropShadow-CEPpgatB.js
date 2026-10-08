import { Ac as defineStd, Ml as p, On as shadowComposite, Xt as childTap, jn as silhouetteCoverage, kn as shadowOffset, sn as gatherStack } from "./std-ClftfWpF.js";
import { a as transformColor, i as transformBoolean } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "DropShadow",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Adds a soft shadow behind the child content based on its alpha silhouette",
	props: {
		color: {
			default: "#000000",
			transform: transformColor,
			description: "Shadow color",
			ui: {
				type: "color",
				label: "Color",
				group: "Colors"
			}
		},
		distance: {
			default: .1,
			description: "How far the shadow is offset from the content",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .005,
				label: "Distance",
				group: "Effect"
			}
		},
		angle: {
			default: 135,
			description: "Direction the shadow is cast (compass degrees: 0=up, 90=right, 135=lower-right, 180=down)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		blur: {
			default: 5,
			description: "Shadow softness (blur radius in pixels)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .5,
				label: "Blur",
				group: "Effect"
			}
		},
		intensity: {
			default: .5,
			description: "Shadow intensity — how strong/visible the shadow is",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Effect"
			}
		},
		cutout: {
			default: false,
			transform: transformBoolean,
			compileTime: true,
			description: "Hide the original layer and show only the shadow",
			ui: {
				type: "checkbox",
				label: "Cutout",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(childTap(), [shadowComposite({
		coverage: silhouetteCoverage({
			at: shadowOffset({
				angle: p("angle"),
				distance: p("distance")
			}),
			blur: p("blur")
		}),
		color: p("color"),
		intensity: p("intensity"),
		cutout: p("cutout")
	})])
});
var DropShadow_default = componentDefinition;
export { componentDefinition as n, DropShadow_default as t };
