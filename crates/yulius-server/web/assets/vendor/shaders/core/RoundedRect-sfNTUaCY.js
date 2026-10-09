import { Ac as defineStd, fs as roundedRect } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "RoundedRect",
	description: "Rounded rectangle with adjustable width, height, and corner rounding",
	shape: {
		distance: ({ x, y }, u) => roundedRect(x, y, u("width"), u("height"), u("rounding")),
		colorDescription: "Fill color of the rectangle",
		centerDescription: "Center position of the rectangle",
		bounds: { size: {
			width: "width",
			height: "height"
		} },
		shapeProps: {
			width: {
				default: .35,
				description: "Half-width of the rectangle",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Width",
					group: "Shape"
				}
			},
			height: {
				default: .25,
				description: "Half-height of the rectangle",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Height",
					group: "Shape"
				}
			},
			rounding: {
				default: .05,
				description: "Corner rounding radius — set to min(width, height) for a pill shape",
				ui: {
					type: "range",
					min: 0,
					max: .5,
					step: .01,
					label: "Rounding",
					group: "Shape",
					dimensional: "canvas-height"
				}
			}
		}
	}
});
var RoundedRect_default = componentDefinition;
export { componentDefinition as n, RoundedRect_default as t };
