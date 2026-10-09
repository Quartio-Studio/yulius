import { Ac as defineStd, os as ellipse } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Ellipse",
	description: "Ellipse with independently adjustable horizontal and vertical radii",
	shape: {
		distance: ({ x, y }, u) => ellipse(x, y, u("radiusX"), u("radiusY")),
		colorDescription: "Fill color of the ellipse",
		centerDescription: "Center position of the ellipse",
		bounds: { size: {
			width: "radiusX",
			height: "radiusY"
		} },
		shapeProps: {
			radiusX: {
				default: .35,
				description: "Horizontal semi-axis radius",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Width",
					group: "Shape"
				}
			},
			radiusY: {
				default: .2,
				description: "Vertical semi-axis radius",
				ui: {
					type: "range",
					min: .01,
					max: 1,
					step: .01,
					label: "Height",
					group: "Shape"
				}
			}
		}
	}
});
var Ellipse_default = componentDefinition;
export { componentDefinition as n, Ellipse_default as t };
