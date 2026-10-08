import { Ac as defineStd, ls as parallelogram } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Parallelogram",
	description: "Parallelogram with adjustable width, height and skew",
	shape: {
		distance: ({ x, y }, u) => parallelogram(x, y, u("width"), u("height"), u("skew")),
		colorDescription: "Fill color of the parallelogram",
		centerDescription: "Center position of the parallelogram",
		bounds: { size: {
			width: "width",
			height: "height"
		} },
		shapeProps: {
			width: {
				default: .32,
				description: "Half-width of the parallelogram",
				ui: {
					type: "range",
					min: .02,
					max: 1,
					step: .01,
					label: "Width",
					group: "Shape"
				}
			},
			height: {
				default: .22,
				description: "Half-height of the parallelogram",
				ui: {
					type: "range",
					min: .02,
					max: 1,
					step: .01,
					label: "Height",
					group: "Shape"
				}
			},
			skew: {
				default: .15,
				description: "Horizontal skew of the top edge",
				ui: {
					type: "range",
					min: -.4,
					max: .4,
					step: .01,
					label: "Skew",
					group: "Shape"
				}
			}
		}
	}
});
var Parallelogram_default = componentDefinition;
export { componentDefinition as n, Parallelogram_default as t };
