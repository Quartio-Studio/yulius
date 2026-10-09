import { Ac as defineStd, Ml as p, Sn as revealShadow, en as curlShading, gn as peelFrame, hn as peelCompose, mn as overhangShadow, sn as gatherStack, tn as curlSheen } from "./std-ClftfWpF.js";
var peel = peelFrame({
	corner: p("corner"),
	amount: p("amount"),
	radius: p("radius")
});
const componentDefinition = defineStd({
	name: "PagePeel",
	role: "filter",
	species: "gather",
	category: "Transitions",
	description: "Curl the content up from a corner like a peeling page",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		corner: {
			default: "bottom-right",
			description: "Which corner the page peels from",
			compileTime: true,
			ui: {
				type: "select",
				options: [
					{
						label: "Top Left",
						value: "top-left"
					},
					{
						label: "Top Right",
						value: "top-right"
					},
					{
						label: "Bottom Left",
						value: "bottom-left"
					},
					{
						label: "Bottom Right",
						value: "bottom-right"
					}
				],
				label: "Corner",
				group: "Effect"
			}
		},
		amount: {
			default: .2,
			description: "How far the peel has progressed across the page (0 = flat, 1 = fully peeled)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Amount",
				group: "Effect"
			}
		},
		radius: {
			default: .2,
			description: "Tightness of the curl (fraction of the page diagonal)",
			ui: {
				type: ["range", "map"],
				min: .02,
				max: .4,
				step: .01,
				label: "Curl Radius",
				group: "Effect"
			}
		},
		shading: {
			default: .55,
			description: "How much the underside of the curl is shaded for depth",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Shading",
				group: "Effect"
			}
		},
		highlight: {
			default: .4,
			description: "Strength of the specular sheen running along the curl",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Highlight",
				group: "Effect"
			}
		},
		highlightSoftness: {
			default: .2,
			description: "Width of the specular highlight band (low = tight gloss, high = broad satin)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Highlight Softness",
				group: "Effect"
			}
		},
		shadow: {
			default: 1,
			description: "Strength of the contact shadow the curl casts on the page",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Shadow",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(peelCompose(peel, {
		shade: curlShading(peel, { shading: p("shading") }),
		sheen: curlSheen(peel, {
			highlight: p("highlight"),
			softness: p("highlightSoftness")
		}),
		overhang: overhangShadow(peel, {
			amount: p("amount"),
			shadow: p("shadow")
		}),
		reveal: revealShadow(peel, {
			amount: p("amount"),
			shadow: p("shadow")
		})
	}), [])
});
var PagePeel_default = componentDefinition;
export { componentDefinition as n, PagePeel_default as t };
