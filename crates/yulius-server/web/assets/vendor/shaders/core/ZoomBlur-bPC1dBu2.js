import { Ac as defineStd, Kt as blurPath, Ml as p, fn as motionBlur } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "ZoomBlur",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Radial zoom blur expanding from a center point",
	props: {
		intensity: {
			default: 30,
			description: "Intensity of the zoom blur effect",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Effect"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center point of the zoom blur",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		}
	},
	effect: motionBlur({
		path: blurPath.zoom(p("center")),
		amount: p("intensity")
	})
});
var ZoomBlur_default = componentDefinition;
export { componentDefinition as n, ZoomBlur_default as t };
